// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::sync::Arc;

use grammers_mtsender::SenderPoolHandle;
use grammers_session::storages::ErasedSession;

pub(crate) struct ClientInner {
    pub(crate) session: Arc<ErasedSession>,
    pub(crate) api_id: i32,
    pub(crate) handle: SenderPoolHandle,
    pub(crate) configuration: ClientConfiguration,
    pub(crate) auth_copied_to_dcs: tokio::sync::Mutex<Vec<i32>>,
}

/// Wrapper around [`SenderPool`] to facilitate interaction with Telegram's API.
///
/// This structure is the "entry point" of the library, from which you can start using the rest.
///
/// This structure owns all the necessary connections to Telegram, and has implementations for the
/// most basic methods, such as connecting, signing in, or processing network events.
///
/// On drop, all state is synchronized to the session. The [`Session`] must be explicitly saved
/// to disk with its corresponding save method for persistence.
///
/// [`SenderPool`]: grammers_mtsender::SenderPool
/// [`Session`]: grammers_session::Session
#[derive(Clone)]
pub struct Client(pub(crate) Arc<ClientInner>);

/// Configuration that controls the [`Client`] behaviour when making requests.
pub struct ClientConfiguration {
    /// Whether to call [`grammers_session::Session::cache_peer`] on all peer information that
    /// the high-level methods receive as a response (e.g. [`Client::iter_dialogs`]).
    ///
    /// The cached peers are then usable by other methods such as [`Client::resolve_peer`]
    /// for as long as the same persisted session is used.
    pub auto_cache_peers: bool,
}

impl Default for ClientConfiguration {
    /// Returns an instance that encountered peers are automatically passed to [`grammers_session::Session::cache_peer`].
    fn default() -> Self {
        Self {
            auto_cache_peers: true,
        }
    }
}
