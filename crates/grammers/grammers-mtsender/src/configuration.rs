// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::num::{NonZeroU32, NonZeroUsize};
use std::ops::ControlFlow;
use std::time::Duration;

use grammers_mtproto::transport::{self, Transport};

use crate::errors::{InvocationError, RpcError};

const DEFAULT_LOCALE: &str = "en";

type TransportFactory = dyn Fn() -> Box<dyn Transport + Send> + Send + Sync;

/// Connection parameters used whenever a new connection is initialized.
///
/// After creating a [`crate::SenderPool::with_configuration`], the connection of
/// any of the [`crate::Sender`]s that it uses internally will be initialized with
/// an instance of [`grammers_tl_types::functions::InitConnection`].
///
/// Some fields are hidden to encourage using the Struct Update Syntax with a default.
pub struct ConnectionParams {
    /// "Device model" according to [`initConnection`](https://core.telegram.org/method/initConnection).
    pub device_model: String,

    /// "Operation system version" according to [`initConnection`](https://core.telegram.org/method/initConnection).
    pub system_version: String,

    /// "Application version" according to [`initConnection`](https://core.telegram.org/method/initConnection).
    pub app_version: String,

    /// Code for the language used on the device's OS, formatted using the ISO 639-1 standard.
    pub system_lang_code: String,

    /// Either an ISO 639-1 language code or a language pack name obtained from
    /// a [language pack link](https://core.telegram.org/api/links#language-pack-links).
    pub lang_code: String,

    /// URL of the proxy to use. Requires the `proxy` feature to be enabled.
    ///
    /// The scheme must be `socks5`. Username and password are optional, e.g.:
    /// - socks5://127.0.0.1:1234
    /// - socks5://username:password@example.com:5678
    ///
    /// Both a host and port must be provided. If a domain is used for the host, its address will be looked up,
    /// and the first IP address found will be used. If a different IP address should be used, consider resolving
    /// the host manually and selecting an IP address of your choice.
    #[cfg(feature = "proxy")]
    pub proxy_url: Option<String>,

    /// Whether to connect via IPv6 instead of defaulting to IPv4.
    pub use_ipv6: bool,

    /// The retry policy to use when encountering errors after invoking a request.
    pub retry_policy: Box<dyn RetryPolicy>,

    /// Maximum number of updates that can be buffered in the internal channel
    /// between the network senders and the updates receiver.
    ///
    /// When the channel is full, newly received updates are **dropped**.
    pub updates_channel_capacity: NonZeroUsize,

    pub transport: Box<TransportFactory>,

    #[doc(hidden)]
    pub __non_exhaustive: (),
}

/// Configuration that controls [`crate::UpdatesReceiver`].
pub struct UpdatesConfiguration {
    /// Should the [`crate::UpdatesReceiver`] catch-up on updates sent to it while it was offline?
    ///
    /// By default, updates sent while the [`crate::UpdatesReceiver`] was offline are ignored.
    pub catch_up: bool,
}

/// This trait controls how the [`SenderPoolRunner`] should behave when
/// an invoked request fails with an [`InvocationError`].
///
/// [`SenderPoolRunner`]: crate::SenderPoolRunner
pub trait RetryPolicy: Send + Sync {
    /// Determines whether the failing request should retry.
    ///
    /// If it should Continue, a sleep duration before retrying is included.\
    /// If it should Break, the context error will be propagated to the caller.
    fn should_retry(&self, ctx: &RetryContext) -> ControlFlow<(), Duration>;
}

/// Context passed to [`RetryPolicy::should_retry`].
pub struct RetryContext {
    /// Amount of times the instance of this request has failed.
    pub fail_count: NonZeroU32,
    /// Sum of the durations for all previous continuations (not total time elapsed since first failure).
    pub slept_so_far: Duration,
    /// The most recent error caused by the instance of the request.
    pub error: InvocationError,
}

/// Retry policy that will never retry.
pub struct NoRetries;

/// Retry policy that will retry up to `tries` times on flood-wait and slow mode wait errors.
///
/// The library will sleep only if the duration to sleep for is below or equal to the threshold.
pub struct AutoSleep {
    /// The (inclusive) number of tries below which the request should be retried.
    pub tries: NonZeroU32,

    /// The (inclusive) threshold below which the library should automatically sleep.
    pub threshold: Duration,

    /// `Some` if I/O errors should be treated as a flood error that would last the specified duration.
    /// This duration will ignore the `threshold` and always be slept on I/O errors while `tries` is not exceeded.
    pub io_errors_as_flood_of: Option<Duration>,
}

impl Default for ConnectionParams {
    /// Returns an instance with an [`AutoSleep::default`] retry policy,
    /// with a capacity of 100 updates.
    ///
    /// [`AutoSleep::default`]: super::AutoSleep::default
    fn default() -> Self {
        let info = os_info::get();

        let mut system_lang_code = String::new();
        let mut lang_code = String::new();

        #[cfg(not(target_os = "android"))]
        {
            system_lang_code.push_str(&locate_locale::system());
            lang_code.push_str(&locate_locale::user());
        }
        if system_lang_code.is_empty() {
            system_lang_code.push_str(DEFAULT_LOCALE);
        }
        if lang_code.is_empty() {
            lang_code.push_str(DEFAULT_LOCALE);
        }

        Self {
            device_model: format!("{} {}", info.os_type(), info.bitness()),
            system_version: info.version().to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            system_lang_code,
            lang_code,
            use_ipv6: false,
            #[cfg(feature = "proxy")]
            proxy_url: None,
            retry_policy: Box::new(super::AutoSleep::default()),
            updates_channel_capacity: NonZeroUsize::new(100).unwrap(),
            transport: Box::new(|| Box::new(transport::Full::new())),
            __non_exhaustive: (),
        }
    }
}

impl Default for UpdatesConfiguration {
    /// Returns an instance that will not catch up.
    fn default() -> Self {
        Self { catch_up: false }
    }
}

impl RetryPolicy for NoRetries {
    fn should_retry(&self, _: &RetryContext) -> ControlFlow<(), Duration> {
        ControlFlow::Break(())
    }
}

impl RetryPolicy for AutoSleep {
    fn should_retry(&self, ctx: &RetryContext) -> ControlFlow<(), Duration> {
        match ctx.error {
            InvocationError::Rpc(RpcError {
                code: 420,
                value: Some(seconds),
                ..
            }) if ctx.fail_count <= self.tries && seconds as u64 <= self.threshold.as_secs() => {
                ControlFlow::Continue(Duration::from_secs(seconds as _))
            }
            InvocationError::Io(_) if ctx.fail_count <= self.tries => {
                if let Some(duration) = self.io_errors_as_flood_of {
                    ControlFlow::Continue(duration)
                } else {
                    ControlFlow::Break(())
                }
            }
            _ => ControlFlow::Break(()),
        }
    }
}

impl Default for AutoSleep {
    /// Returns an instance with a threshold of 60 seconds.
    ///
    /// I/O errors will be treated as if they were a 1-second flood.
    fn default() -> Self {
        Self {
            tries: NonZeroU32::MIN,
            threshold: Duration::from_secs(60),
            io_errors_as_flood_of: Some(Duration::from_secs(1)),
        }
    }
}
