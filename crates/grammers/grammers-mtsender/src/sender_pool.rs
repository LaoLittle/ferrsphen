// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
use std::num::NonZeroU32;
use std::ops::{ControlFlow, Deref};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{fmt, panic};

use grammers_mtproto::{mtp, transport};
use grammers_session::Session;
use grammers_session::storages::{ErasedSession, erase};
use grammers_session::types::DcOption;
use grammers_session::updates::UpdatesLike;
use grammers_tl_types::{self as tl, Deserializable, enums};
use tokio::task::AbortHandle;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::sleep,
};
use tracing::{debug, info, warn};

use crate::configuration::{ConnectionParams, RetryContext};
use crate::errors::{InvocationError, ReadError};
use crate::{Sender, ServerAddr, connect, connect_with_auth};

/// How long to wait after warning the user that the updates was dropped.
const UPDATES_DROPPED_LOG_COOLDOWN: Duration = Duration::from_secs(300);

type InvokeResponse = Vec<u8>;

enum Request {
    Invoke {
        dc_id: i32,
        body: Vec<u8>,
        tx: oneshot::Sender<Result<InvokeResponse, InvocationError>>,
    },
    CheckRetry {
        error: InvocationError,
        fail_count: NonZeroU32,
        slept_so_far: Duration,
        tx: oneshot::Sender<ControlFlow<InvocationError, Duration>>,
    },
    Disconnect {
        dc_id: i32,
    },
    Quit,
}

struct Rpc {
    body: Vec<u8>,
    tx: oneshot::Sender<Result<InvokeResponse, InvocationError>>,
}

struct ConnectionInfo {
    dc_id: i32,
    rpc_tx: mpsc::UnboundedSender<Rpc>,
    abort_handle: AbortHandle,
}

/// A fat [`SenderPoolHandle`] with additional metadata from its attached [`SenderPoolRunner`].
#[derive(Clone)]
pub struct SenderPoolFatHandle {
    /// The inner thin handle that self can be derefed into.
    ///
    /// The rest of fields can be dropped if they are no longer needed.
    pub thin: SenderPoolHandle,
    /// The session in use by the attached [`SenderPoolRunner`].
    ///
    /// The runner will read and persist datacenter options in it.
    pub session: Arc<ErasedSession>,
    /// Developer's [Application Identifier](https://core.telegram.org/myapp).
    ///
    /// The [`SenderPoolRunner`] will make use of this value when it needs
    /// to invoke [`tl::functions::InitConnection`] after creating a new connection.
    pub api_id: i32,
}

/// Cheaply cloneable handle to interact with its [`SenderPoolRunner`].
#[derive(Clone)]
pub struct SenderPoolHandle(mpsc::UnboundedSender<Request>);

/// Builder to configure the runner to drive I/O and linked handles.
pub struct SenderPool {
    /// The single mutable instance responsible for driving I/O.
    ///
    /// Connections are created on-demand, so any errors while the pool
    /// is running can only be retrieved with one of the [`SenderPool::handle`]s.
    pub runner: SenderPoolRunner,
    /// Starting fat handle attached to the [`SenderPool::runner`].
    ///
    /// Handles are the only way to interact with the runner once it's running.
    pub handle: SenderPoolFatHandle,
    /// The single mutable channel through which updates received
    /// from the network by the [`SenderPool::runner`] are delivered.
    ///
    /// Update handling must be processed in a sequential manner,
    /// so this is a separate instance with no way to clone it.
    pub updates: mpsc::Receiver<UpdatesLike>,
}

/// Manages and runs a pool of zero or more [`Sender`]s.
///
/// Use [`SenderPool::new`] to create an instance of this type and associated channels.
pub struct SenderPoolRunner {
    session: Arc<ErasedSession>,
    api_id: i32,
    connection_params: ConnectionParams,
    request_rx: mpsc::UnboundedReceiver<Request>,
    updates_tx: mpsc::Sender<UpdatesLike>,
    connections: Vec<ConnectionInfo>,
    connection_pool: JoinSet<Result<(), ReadError>>,
}

impl Deref for SenderPoolFatHandle {
    type Target = SenderPoolHandle;

    fn deref(&self) -> &Self::Target {
        &self.thin
    }
}

impl SenderPoolHandle {
    /// Communicate with the running [`SenderPoolRunner`] instance
    /// to invoke the request in the specified datacenter.
    pub async fn invoke_in_dc<R: tl::RemoteCall>(
        &self,
        dc_id: i32,
        request: &R,
    ) -> Result<R::Return, InvocationError> {
        self.do_invoke_in_dc(dc_id, request.to_bytes())
            .await
            .and_then(|body| R::Return::from_bytes(&body).map_err(|e| e.into()))
    }

    /// Communicate with the running [`SenderPoolRunner`] instance
    /// to invoke the serialized request body in the specified datacenter.
    pub async fn raw_invoke_in_dc(
        &self,
        dc_id: i32,
        body: Vec<u8>,
    ) -> Result<InvokeResponse, InvocationError> {
        let (tx, rx) = oneshot::channel();
        self.0
            .send(Request::Invoke { dc_id, body, tx })
            .map_err(|_| InvocationError::Dropped("SenderPoolRunner was dropped at send"))?;
        rx.await
            .map_err(|_| InvocationError::Dropped("SenderPoolRunner was dropped during receive"))?
    }

    /// Communicate with the running [`SenderPoolRunner`] instance
    /// to drop any active connections to the given datacenter.
    ///
    /// Has no effect if there was no connection to the datacenter.
    ///
    /// This is useful after datacenter migrations during sign in,
    /// when the old connection is known to not be needed anymore.
    pub fn disconnect_from_dc(&self, dc_id: i32) -> bool {
        self.0.send(Request::Disconnect { dc_id }).is_ok()
    }

    /// Communicate with the running [`SenderPoolRunner`] instance
    /// to drop all active connections and gracefully stop running.
    pub fn quit(&self) -> bool {
        self.0.send(Request::Quit).is_ok()
    }

    /// do [`Self::raw_invoke_in_dc`] with [`Self::check_retry`]
    pub(crate) async fn do_invoke_in_dc(
        &self,
        dc_id: i32,
        request_body: Vec<u8>,
    ) -> Result<Vec<u8>, InvocationError> {
        let mut fail_count = NonZeroU32::new(1).unwrap();
        let mut slept_so_far = Duration::default();

        loop {
            match self.raw_invoke_in_dc(dc_id, request_body.clone()).await {
                Ok(response) => break Ok(response),
                Err(e) => {
                    let error_info = format!("{}", e);
                    match self.check_retry(e, fail_count, slept_so_far).await {
                        ControlFlow::Continue(delay) => {
                            info!(?delay, "sleeping on {} before retrying", error_info);
                            sleep(delay).await;
                            fail_count = fail_count.saturating_add(1);
                            slept_so_far += delay;
                            continue;
                        }
                        ControlFlow::Break(final_error) => break Err(final_error),
                    }
                }
            }
        }
    }

    /// Communicate with the running [`SenderPoolRunner`] instance
    /// to check whether the request needs to retry.
    async fn check_retry(
        &self,
        error: InvocationError,
        fail_count: NonZeroU32,
        slept_so_far: Duration,
    ) -> ControlFlow<InvocationError, Duration> {
        let (tx, rx) = oneshot::channel();
        if self
            .0
            .send(Request::CheckRetry {
                error,
                fail_count,
                slept_so_far,
                tx,
            })
            .is_err()
        {
            return ControlFlow::Break(InvocationError::Dropped(
                "SenderPoolRunner was dropped during retry at send",
            ));
        }

        match rx.await {
            Ok(flow) => flow,
            Err(_) => ControlFlow::Break(InvocationError::Dropped(
                "SenderPooLRunner was dropped during retry during receive",
            )),
        }
    }
}

impl SenderPool {
    /// Creates a new sender pool instance with default configuration,
    /// attached to the given session and using the provided
    /// [Application Identifier](https://core.telegram.org/myapp)
    /// belonging to the developer.
    ///
    /// Session instance **should not** be reused by multiple pools at the same time.
    /// The session instance will only be used to query datacenter options and persist
    /// any permanent Authorization Keys generated for previously-unconncected datacenters.
    pub fn new<S>(session: Arc<S>, api_id: i32) -> Self
    where
        S: Session + Sized,
        S::Error: std::error::Error + Send + Sync + 'static,
    {
        Self::with_configuration(session, api_id, Default::default())
    }

    /// Creates a new sender pool with non-[`ConnectionParams::default`] configuration.
    pub fn with_configuration<S>(
        session: Arc<S>,
        api_id: i32,
        connection_params: ConnectionParams,
    ) -> Self
    where
        S: Session + Sized,
        S::Error: std::error::Error + Send + Sync + 'static,
    {
        let session = erase(session);
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let (updates_tx, updates_rx) =
            mpsc::channel(connection_params.updates_channel_capacity.get());

        Self {
            runner: SenderPoolRunner {
                session: Arc::clone(&session),
                api_id,
                connection_params,
                request_rx,
                updates_tx,
                connections: Vec::new(),
                connection_pool: JoinSet::new(),
            },
            handle: SenderPoolFatHandle {
                thin: SenderPoolHandle(request_tx),
                session: Arc::clone(&session),
                api_id,
            },
            updates: updates_rx,
        }
    }
}

impl SenderPoolRunner {
    /// Run the sender pool until [`SenderPoolHandle::quit`] is called or the returned future is dropped.
    ///
    /// Connections will be initiated on-demand whenever the first request to a datacenter is made.
    pub async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                completion = self.connection_pool.join_next(), if !self.connection_pool.is_empty() => {
                    if let Err(err) = completion.unwrap()
                        && let Ok(reason) = err.try_into_panic()
                    {
                        panic::resume_unwind(reason);
                    }
                    self.connections
                        .retain(|connection| !connection.abort_handle.is_finished());
                }
                request = self.request_rx.recv() => {
                    let flow = if let Some(request) = request {
                        self.process_request(request).await
                    } else {
                        ControlFlow::Break(())
                    };
                    match flow {
                        ControlFlow::Continue(_) => continue,
                        ControlFlow::Break(_) => break,
                    }
                }
            }
        }

        self.connections.clear(); // drop all channels to cause the `run_sender` loops to stop
        self.connection_pool.join_all().await;
    }

    async fn process_request(&mut self, request: Request) -> ControlFlow<()> {
        match request {
            Request::Invoke { dc_id, body, tx } => {
                let connection = match self
                    .connections
                    .iter()
                    .find(|connection| connection.dc_id == dc_id)
                {
                    Some(connection) => connection,
                    None => match self.create_connection(dc_id).await {
                        Ok(x) => x,
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            return ControlFlow::Continue(());
                        }
                    },
                };
                let _ = connection.rpc_tx.send(Rpc { body, tx });
                ControlFlow::Continue(())
            }
            Request::CheckRetry {
                error,
                fail_count,
                slept_so_far,
                tx,
            } => {
                let retry_context = RetryContext {
                    fail_count,
                    slept_so_far,
                    error,
                };
                let flow = match self
                    .connection_params
                    .retry_policy
                    .should_retry(&retry_context)
                {
                    ControlFlow::Continue(delay) => ControlFlow::Continue(delay),
                    ControlFlow::Break(()) => ControlFlow::Break(retry_context.error),
                };
                let _ = tx.send(flow);
                ControlFlow::Continue(())
            }
            Request::Disconnect { dc_id } => {
                self.connections.retain(|connection| {
                    if connection.dc_id == dc_id {
                        connection.abort_handle.abort();
                        false
                    } else {
                        true
                    }
                });
                ControlFlow::Continue(())
            }
            Request::Quit => ControlFlow::Break(()),
        }
    }

    async fn create_connection(&mut self, dc_id: i32) -> Result<&ConnectionInfo, InvocationError> {
        let mut dc_option = match self.session.dc_option(dc_id)? {
            Some(x) => x,
            None => return Err(InvocationError::InvalidDc),
        };

        let sender = self.connect_sender(&dc_option).await?;

        dc_option.auth_key = Some(sender.auth_key());
        self.session.set_dc_option(&dc_option).await?;

        let (rpc_tx, rpc_rx) = mpsc::unbounded_channel();
        let abort_handle = self.connection_pool.spawn(run_sender(
            sender,
            rpc_rx,
            self.updates_tx.clone(),
            dc_option.id == self.session.home_dc_id()?,
        ));
        self.connections.push(ConnectionInfo {
            dc_id,
            rpc_tx,
            abort_handle,
        });
        Ok(self.connections.last().unwrap())
    }

    async fn connect_sender(
        &mut self,
        dc_option: &DcOption,
    ) -> Result<Sender<Box<dyn transport::Transport + Send>, mtp::Encrypted>, InvocationError> {
        let transport = &self.connection_params.transport;

        let address = if self.connection_params.use_ipv6 {
            dc_option.ipv6.into()
        } else {
            dc_option.ipv4.into()
        };

        #[cfg(feature = "proxy")]
        let addr = || {
            if let Some(proxy) = self.connection_params.proxy_url.clone() {
                ServerAddr::Proxied { address, proxy }
            } else {
                ServerAddr::Tcp { address }
            }
        };
        #[cfg(not(feature = "proxy"))]
        let addr = || ServerAddr::Tcp { address };

        let init_connection = tl::functions::InvokeWithLayer {
            layer: tl::LAYER,
            query: tl::functions::InitConnection {
                api_id: self.api_id,
                device_model: self.connection_params.device_model.clone(),
                system_version: self.connection_params.system_version.clone(),
                app_version: self.connection_params.app_version.clone(),
                system_lang_code: self.connection_params.system_lang_code.clone(),
                lang_pack: "".into(),
                lang_code: self.connection_params.lang_code.clone(),
                proxy: None,
                params: None,
                query: tl::functions::help::GetConfig {},
            },
        };

        let mut sender = if let Some(auth_key) = dc_option.auth_key {
            connect_with_auth(transport(), addr(), auth_key)
                .await
                .map_err(InvocationError::Io)?
        } else {
            connect(transport(), addr()).await?
        };

        let enums::Config::Config(remote_config) = match sender.invoke(&init_connection).await {
            Ok(config) => config,
            Err(InvocationError::Transport(transport::Error::BadStatus { status: 404 })) => {
                sender = connect(transport(), addr()).await?;
                sender.invoke(&init_connection).await?
            }
            Err(e) => return Err(e),
        };

        self.update_config(remote_config).await?;

        Ok(sender)
    }

    async fn update_config(
        &self,
        config: tl::types::Config,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for option in config
            .dc_options
            .iter()
            .map(|tl::enums::DcOption::Option(option)| option)
            .filter(|option| !option.media_only && !option.tcpo_only && option.r#static)
        {
            let mut dc_option = self
                .session
                .dc_option(option.id)?
                .unwrap_or_else(|| DcOption {
                    id: option.id,
                    ipv4: SocketAddrV4::new(Ipv4Addr::from_bits(0), 0),
                    ipv6: SocketAddrV6::new(Ipv6Addr::from_bits(0), 0, 0, 0),
                    auth_key: None,
                });
            if option.ipv6 {
                dc_option.ipv6 = SocketAddrV6::new(
                    option
                        .ip_address
                        .parse()
                        .expect("Telegram to return a valid IPv6 address"),
                    option.port as _,
                    0,
                    0,
                );
            } else {
                dc_option.ipv4 = SocketAddrV4::new(
                    option
                        .ip_address
                        .parse()
                        .expect("Telegram to return a valid IPv4 address"),
                    option.port as _,
                );
                if dc_option.ipv6.ip().to_bits() == 0 {
                    dc_option.ipv6 = SocketAddrV6::new(
                        dc_option.ipv4.ip().to_ipv6_mapped(),
                        dc_option.ipv4.port(),
                        0,
                        0,
                    )
                }
            }
            self.session.set_dc_option(&dc_option).await?;
        }
        Ok(())
    }
}

async fn run_sender(
    mut sender: Sender<Box<dyn transport::Transport + Send>, grammers_mtproto::mtp::Encrypted>,
    mut rpc_rx: mpsc::UnboundedReceiver<Rpc>,
    updates: mpsc::Sender<UpdatesLike>,
    home_sender: bool,
) -> Result<(), ReadError> {
    let mut dropped: usize = 0;
    let mut last_update_limit_warn = Instant::now();
    let mut updates_closed = false;

    loop {
        tokio::select! {
            step = sender.step() => match step {
                Ok(all_new_updates) => {
                    if updates_closed {
                        continue;
                    }
                    for new_updates in all_new_updates {
                        if let Err(e) = updates.try_send(new_updates) {
                            match e {
                                mpsc::error::TrySendError::Full(_) => {
                                    dropped += 1;
                                    if last_update_limit_warn.elapsed() >= UPDATES_DROPPED_LOG_COOLDOWN {
                                        warn!(
                                            capacity = updates.max_capacity(),
                                            dropped_count = dropped,
                                            last_time = ?last_update_limit_warn.elapsed(),
                                            "updates channel full",
                                        );
                                        dropped = 0;
                                        last_update_limit_warn = Instant::now();
                                    }
                                }
                                mpsc::error::TrySendError::Closed(_) => {
                                    debug!("updates channel closed, stopping update forwarding");
                                    updates_closed = true;
                                    break;
                                }
                            }
                        }
                    }
                },
                Err(err) => {
                    if home_sender {
                        let _ = updates.try_send(UpdatesLike::ConnectionClosed);
                    }
                    break Err(err)
                },
            },
            rpc = rpc_rx.recv() => match rpc {
                Some(rpc) => sender.enqueue_body(rpc.body, rpc.tx),
                None => break Ok(()),
            },
        }
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invoke { dc_id, body, tx } => f
                .debug_struct("Invoke")
                .field("dc_id", dc_id)
                .field(
                    "request",
                    &body[..4]
                        .try_into()
                        .map(|constructor_id| tl::name_for_id(u32::from_le_bytes(constructor_id)))
                        .unwrap_or("?"),
                )
                .field("tx", tx)
                .finish(),
            Self::CheckRetry {
                error,
                fail_count,
                slept_so_far,
                tx,
            } => f
                .debug_struct("CheckRetry")
                .field("error", error)
                .field("fail_count", fail_count)
                .field("slept_so_far", slept_so_far)
                .field("tx", tx)
                .finish(),
            Self::Disconnect { dc_id } => {
                f.debug_struct("Disconnect").field("dc_id", dc_id).finish()
            }
            Self::Quit => write!(f, "Quit"),
        }
    }
}
