// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime};
use std::{io, thread};

use grammers_mtproto::mtp;
use grammers_mtproto::transport::Transport;
use grammers_mtproto::{MsgId, authentication, connection};
use grammers_session::updates::UpdatesLike;
use grammers_tl_types::{self as tl, Deserializable, RemoteCall};
use tl::Serializable;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::sync::oneshot::error::TryRecvError;
use tokio::time::{Instant, sleep_until};
use tracing::{debug, error, info, trace, warn};

use crate::RpcError;
use crate::errors::{InvocationError, ReadError};
use crate::net::{NetStream, ServerAddr};

/// Every how often are pings sent?
const PING_DELAY: Duration = Duration::from_secs(60);

/// After how many seconds should the server close the connection when we send a ping?
///
/// What this value essentially means is that we have `NO_PING_DISCONNECT - PING_DELAY` seconds
/// to keep sending pings, or the server will close the connection.
///
/// Pings ensure the connection is kept active, and the delayed disconnect ensures the messages
/// are getting through consistently enough.
const NO_PING_DISCONNECT: i32 = 75;

/// Generate a "random" ping ID.
pub(crate) fn generate_random_id() -> i64 {
    static LAST_ID: AtomicI64 = AtomicI64::new(0);

    while LAST_ID.load(Ordering::SeqCst) == 0 {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system time is before epoch")
            .as_nanos() as i64;

        if LAST_ID
            .compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            thread::yield_now();
        }
    }

    LAST_ID.fetch_add(1, Ordering::SeqCst)
}

/// Manages enqueuing requests, matching them to their response, and IO.
pub struct Sender<T: Transport, M: mtp::Mtp> {
    stream: NetStream,
    addr: ServerAddr,
    state: connection::State<T, M>,
    requests: Vec<Request>,
    next_ping: Instant,
}

struct Request {
    body: Vec<u8>,
    state: RequestState,
    result: oneshot::Sender<Result<Vec<u8>, InvocationError>>,
}

#[derive(Clone, Debug)]
struct MsgIdPair {
    msg_id: MsgId,
    container_msg_id: MsgId,
}

enum RequestState {
    NotSerialized,
    Serialized(MsgIdPair),
    Sent(MsgIdPair),
}

impl MsgIdPair {
    fn new(msg_id: MsgId) -> Self {
        Self {
            msg_id,
            container_msg_id: msg_id, // by default, no container (so the last msg_id is itself)
        }
    }
}

impl<T: Transport, M: mtp::Mtp> Sender<T, M> {
    /// Create a new connection using the specified transport, MTP state and server address.
    ///
    /// Note that this does not attempt to invoke [`tl::functions::InitConnection`].
    /// It will simply open a new socket connection to the provided address.
    pub async fn connect(transport: T, mtp: M, addr: ServerAddr) -> Result<Self, io::Error> {
        let stream = NetStream::connect(&addr).await?;
        Ok(Self {
            stream,
            addr,
            state: connection::State::new(transport, mtp),
            requests: vec![],
            next_ping: Instant::now() + PING_DELAY,
        })
    }

    /// Shutdown the connection.
    ///
    /// Any further [`Self::invoke`] or [`Self::step`] will return an error.
    /// This method is useful if one wants to explicitly handle on-close errors.
    pub async fn disconnect(&mut self) -> io::Result<()> {
        self.stream.disconnect().await
    }

    /// Serializes the given request, enqueues it to the internal buffer,
    /// and repeatedly calls [`Self::step`] until a response for it is received.
    pub async fn invoke<R: RemoteCall>(
        &mut self,
        request: &R,
    ) -> Result<R::Return, InvocationError> {
        let (tx, rx) = oneshot::channel();
        self.enqueue_body(request.to_bytes(), tx);
        self.step_until_receive(rx)
            .await
            .and_then(|vec| R::Return::from_bytes(&vec).map_err(|err| err.into()))
    }

    pub(crate) fn enqueue_body(
        &mut self,
        body: Vec<u8>,
        tx: oneshot::Sender<Result<Vec<u8>, InvocationError>>,
    ) {
        assert!(body.len() >= 4);
        let req_id = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        debug!(
            "enqueueing request {} to be serialized",
            tl::name_for_id(req_id)
        );

        self.requests.push(Request {
            body,
            state: RequestState::NotSerialized,
            result: tx,
        });
    }

    async fn step_until_receive(
        &mut self,
        mut rx: oneshot::Receiver<Result<Vec<u8>, InvocationError>>,
    ) -> Result<Vec<u8>, InvocationError> {
        loop {
            self.step().await?;
            match rx.try_recv() {
                Ok(x) => break x,
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Closed) => {
                    panic!("request channel dropped before receiving a result")
                }
            }
        }
    }

    /// Step network events, writing and reading at the same time.
    ///
    /// Updates received during this step, if any, are returned.
    ///
    /// If an error is returned, the connection should be treated
    /// as dead and the sender instance recreated.
    pub async fn step(&mut self) -> Result<Vec<UpdatesLike>, ReadError> {
        self.try_fill_write();

        let (mut reader, mut writer) = self.stream.split();
        let sleep = sleep_until(self.next_ping);
        let (read, write) = self.state.io_buffers();

        trace!(
            "reading bytes and sending up to {} bytes via network",
            write.len()
        );
        let res = tokio::select! {
            n = reader.read(read) => {
                n.map_err(ReadError::Io).and_then(|n| self.on_net_read(n))
            }
            n = writer.write(write), if !write.is_empty() => {
                n.map_err(ReadError::Io).map(|n| {
                    self.on_net_write(n);
                    Vec::new()
                })
            }
            _ = sleep => {
                self.on_ping_timeout();
                Ok(Vec::new())
            }
        };

        match res {
            Ok(ok) => Ok(ok),
            Err(err) => {
                self.on_error(&err);
                Err(err)
            }
        }
    }

    /// Setup the write buffer for the transport, unless a write is already pending.
    fn try_fill_write(&mut self) {
        if !self.state.can_buffer_requests() {
            return;
        }

        // TODO add a test to make sure we only ever send the same request once
        for request in self
            .requests
            .iter_mut()
            .filter(|r| matches!(r.state, RequestState::NotSerialized))
        {
            if let Some(msg_id) = self.state.buffer_request(&request.body) {
                // Note how only NotSerialized become Serialized.
                // Nasty bugs that take ~2h to find occur otherwise!
                // (e.g. infinite loops leading to transport flood.)
                request.state = RequestState::Serialized(MsgIdPair::new(msg_id));
            } else {
                break;
            }
        }

        if let Some(container_msg_id) = self.state.pack_outgoing_buffer() {
            for request in self.requests.iter_mut() {
                match request.state {
                    RequestState::Serialized(ref mut pair) => {
                        pair.container_msg_id = container_msg_id;
                    }
                    RequestState::NotSerialized | RequestState::Sent(..) => {}
                }
            }
        }
    }

    /// Handle `n` more read bytes being ready to process by the transport.
    ///
    /// This won't cause `ReadError::Io`, but yet another enum would be overkill.
    fn on_net_read(&mut self, n: usize) -> Result<Vec<UpdatesLike>, ReadError> {
        self.state.handle_net_read(n)?;

        // TODO the buffer might have multiple transport packets, what should happen with the
        // updates successfully read if subsequent packets fail to be deserialized properly?
        let mut updates = Vec::new();
        while let Some(result) = self.state.unpack_next() {
            self.process_mtp_buffer(result?, &mut updates);
        }
        Ok(updates)
    }

    /// Handle `n` more written bytes being ready to process by the transport.
    fn on_net_write(&mut self, n: usize) {
        if self.state.handle_net_write(n) {
            for req in self.requests.iter_mut() {
                match &req.state {
                    RequestState::NotSerialized | RequestState::Sent(_) => {}
                    RequestState::Serialized(pair) => {
                        debug!(?pair, "sent request with");
                        req.state = RequestState::Sent(pair.clone());
                    }
                }
            }
        }
    }

    /// Handle a ping timeout, meaning we need to enqueue a new ping request.
    fn on_ping_timeout(&mut self) {
        let ping_id = generate_random_id();
        debug!("enqueueing keepalive ping {}", ping_id);
        let (tx, _rx) = oneshot::channel();
        self.enqueue_body(
            tl::functions::PingDelayDisconnect {
                ping_id,
                disconnect_delay: NO_PING_DISCONNECT,
            }
            .to_bytes(),
            tx,
        );
        self.next_ping = Instant::now() + PING_DELAY;
    }

    /// Handle errors that occured while performing I/O.
    fn on_error(&mut self, error: &ReadError) {
        warn!(
            "marking all {} request(s) as failed: {}",
            self.requests.len(),
            &error
        );

        self.requests
            .drain(..)
            .for_each(|r| drop(r.result.send(Err(InvocationError::from(error.clone())))));
    }

    /// Process the result of deserializing an MTP buffer.
    fn process_mtp_buffer(
        &mut self,
        results: Vec<mtp::Deserialization>,
        updates: &mut Vec<UpdatesLike>,
    ) {
        for result in results {
            match result {
                mtp::Deserialization::OwnUpdate { msg_id, update } => {
                    self.process_own_update(updates, msg_id, update)
                }
                mtp::Deserialization::Update(update) => self.process_update(updates, update),
                mtp::Deserialization::RpcResult(result) => self.process_result(result),
                mtp::Deserialization::RpcError(error) => self.process_error(error),
                mtp::Deserialization::BadMessage(bad_msg) => self.process_bad_message(bad_msg),
                mtp::Deserialization::Failure(failure) => self.process_deserialize_error(failure),
            }
        }
    }

    fn process_own_update(
        &mut self,
        updates: &mut Vec<UpdatesLike>,
        msg_id: MsgId,
        update: Vec<u8>,
    ) {
        match (
            UpdatesLike::from_bytes(&update),
            self.peek_request(msg_id).and_then(|request| {
                tl::functions::messages::SendMessage::from_boxed_bytes(&request.body).ok()
            }),
        ) {
            (
                Ok(UpdatesLike::Updates(tl::enums::Updates::UpdateShortSentMessage(u))),
                Some(request),
            ) => {
                // As far as I know, UpdateShortSentMessage can only occur from SendMessage.
                // If that's not the case, new variants with additional requests should be added.
                updates.push(UpdatesLike::ShortSentMessage { request, update: u })
            }
            (Ok(UpdatesLike::AffectedMessages(affected)), _) => {
                // Check if the original request targeted a channel (e.g. channels.deleteMessages).
                // If so, the pts belongs to that channel, not the common/user pts sequence.
                let channel_id = self
                    .peek_request(msg_id)
                    .and_then(|request| {
                        tl::functions::channels::DeleteMessages::from_boxed_bytes(&request.body)
                            .ok()
                    })
                    .and_then(|request| match request.channel {
                        tl::enums::InputChannel::Channel(c) => Some((c.channel_id, request.id)),
                        _ => None,
                    });
                if let Some((channel_id, message_ids)) = channel_id {
                    updates.push(UpdatesLike::AffectedChannelMessages {
                        affected,
                        channel_id,
                        message_ids,
                    });
                } else {
                    updates.push(UpdatesLike::AffectedMessages(affected));
                }
            }
            (Ok(u), _) => {
                // In the future, we might want to flag "updates produced by the client" somehow.
                // This would be the starting place to do it.
                updates.push(u);
            }
            (Err(e), _) => {
                // This shouldn't happen. We just made the request, so they shouldn't be stale.
                // If the request failed with a failed-to-deserialize error, callers will handle it.
                warn!("telegram responded with updates that failed to be deserialized: {e}")
            }
        }
    }

    fn process_update(&mut self, updates: &mut Vec<UpdatesLike>, update: Vec<u8>) {
        match UpdatesLike::from_bytes(&update) {
            Ok(u) => updates.push(u),
            Err(e) => {
                // These are to be treated as a gap.
                // > Manually obtaining updates through [get difference] is required in the following situations:
                // > […] Incorrect update: the client cannot deserialize the received data.
                warn!("telegram sent updates that failed to be deserialized: {e}");
                updates.push(UpdatesLike::MalformedUpdates);
            }
        }
    }

    fn process_result(&mut self, result: mtp::RpcResult) {
        if let Some(req) = self.pop_request(result.msg_id) {
            let x = result.body;
            assert!(x.len() >= 4);
            let res_id = u32::from_le_bytes([x[0], x[1], x[2], x[3]]);
            debug!(
                res_id = format_args!("{:x}", res_id),
                res_name = tl::name_for_id(res_id),
                request = ?result.msg_id,
                "got result",
            );
            drop(req.result.send(Ok(x)));
        } else {
            info!(
                result = ?result.msg_id,
                "got rpc result but no such request is saved",
            );
        }
    }

    fn process_error(&mut self, error: mtp::RpcResultError) {
        if let Some(req) = self.pop_request(error.msg_id) {
            debug!(error = ?error.error, "got rpc error");
            let x = req.body.as_slice();
            drop(
                req.result.send(Err(InvocationError::Rpc(
                    RpcError::from(error.error)
                        .with_caused_by(u32::from_le_bytes([x[0], x[1], x[2], x[3]])),
                ))),
            );
        } else {
            info!(
                error = ?error.msg_id,
                "got rpc error but no such request is saved",
            );
        }
    }

    fn process_bad_message(&mut self, bad_msg: mtp::BadMessage) {
        for i in (0..self.requests.len()).rev() {
            match &self.requests[i].state {
                RequestState::Serialized(pair)
                    if pair.msg_id == bad_msg.msg_id || pair.container_msg_id == bad_msg.msg_id =>
                {
                    panic!(
                        "bad msg for unsent request {:?}: {}",
                        bad_msg.msg_id,
                        bad_msg.description()
                    );
                }
                RequestState::Sent(pair)
                    if pair.msg_id == bad_msg.msg_id || pair.container_msg_id == bad_msg.msg_id =>
                {
                    // TODO add a test to make sure we resend the request
                    if bad_msg.retryable() {
                        info!(
                            request = ?pair.msg_id,
                            "{}; re-sending request",
                            bad_msg.description(),
                        );

                        // TODO check if actually retryable first!
                        self.requests[i].state = RequestState::NotSerialized;
                    } else {
                        if bad_msg.fatal() {
                            error!(
                                request = ?pair.msg_id,
                                "{}; cannot retry request",
                                bad_msg.description(),
                            );
                        } else {
                            warn!(
                                request = ?pair.msg_id,
                                "{}; cannot retry request",
                                bad_msg.description(),
                            );
                        }
                        let req = self.requests.swap_remove(i);
                        drop(req.result.send(Err(InvocationError::Dropped(
                            "response was non-retryable bad msg",
                        ))));
                    }
                }
                _ => {}
            }
        }
    }

    fn process_deserialize_error(&mut self, failure: mtp::DeserializationFailure) {
        if let Some(req) = self.pop_request(failure.msg_id) {
            debug!(error = ?failure.error, "got deserialization failure");
            drop(req.result.send(Err(InvocationError::from(failure.error))));
        } else {
            info!(
                error = ?failure.error,
                "got deserialization failure but no such request is saved",
            );
        }
    }

    fn peek_request(&mut self, msg_id: MsgId) -> Option<&Request> {
        self.requests.iter().find(|request| match request.state {
            RequestState::NotSerialized => false,
            RequestState::Serialized(MsgIdPair { msg_id: m, .. }) => m == msg_id,
            RequestState::Sent(MsgIdPair { msg_id: m, .. }) => m == msg_id,
        })
    }

    fn pop_request(&mut self, msg_id: MsgId) -> Option<Request> {
        for i in 0..self.requests.len() {
            match &self.requests[i].state {
                RequestState::Serialized(pair) if pair.msg_id == msg_id => {
                    panic!("got response {msg_id:?} for unsent request {pair:?}");
                }
                RequestState::Sent(pair) if pair.msg_id == msg_id => {
                    return Some(self.requests.swap_remove(i));
                }
                _ => {}
            }
        }

        None
    }
}

impl<T: Transport> Sender<T, mtp::Encrypted> {
    pub fn auth_key(&self) -> [u8; 256] {
        self.state.mtp.auth_key()
    }
}

/// Helper function to [`Sender::connect`] a plain transport and [`generate_auth_key`] on it.
pub async fn connect<T: Transport>(
    transport: T,
    addr: ServerAddr,
) -> Result<Sender<T, mtp::Encrypted>, InvocationError> {
    let sender = Sender::connect(transport, mtp::Plain::new(), addr).await?;
    generate_auth_key(sender).await
}

/// Uses the input plain sender to carry the Authorization Key generation process,
/// and returns an encrypted sender reusing the same connection, transport and buffers.
pub async fn generate_auth_key<T: Transport>(
    mut sender: Sender<T, mtp::Plain>,
) -> Result<Sender<T, mtp::Encrypted>, InvocationError> {
    info!("generating new authorization key...");
    let (request, data) = authentication::step1()?;
    debug!("gen auth key: sending step 1");
    let response = sender.invoke(&request).await?;
    debug!("gen auth key: starting step 2");
    let (request, data) = authentication::step2(data, response)?;
    debug!("gen auth key: sending step 2");
    let response = sender.invoke(&request).await?;
    debug!("gen auth key: starting step 3");
    let (request, data) = authentication::step3(data, response)?;
    debug!("gen auth key: sending step 3");
    let response = sender.invoke(&request).await?;
    debug!("gen auth key: completing generation");
    let auth = authentication::create_key(data, response)?;
    info!("authorization key generated successfully");

    Ok(Sender {
        stream: sender.stream,
        addr: sender.addr,
        state: sender.state.into_encrypted(auth),
        requests: sender.requests,
        next_ping: Instant::now() + PING_DELAY,
    })
}

/// Helper function to [`Sender::connect`] an encrypted transport with a previous Authorization Key.
pub async fn connect_with_auth<T: Transport>(
    transport: T,
    addr: ServerAddr,
    auth_key: [u8; 256],
) -> Result<Sender<T, mtp::Encrypted>, io::Error> {
    Sender::connect(transport, mtp::Encrypted::build().finish(auth_key), addr).await
}
