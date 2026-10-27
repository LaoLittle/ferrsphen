// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Methods to deal with and offer access to updates.

use std::collections::VecDeque;

use grammers_mtsender::{InvocationError, UpdatesConfiguration, UpdatesReceiver};
use grammers_session::updates::{State, UpdatesLike};
use grammers_tl_types as tl;
use tokio::sync::mpsc;

use super::Client;
use crate::peer::PeerMap;
use crate::update::Update;

/// Iterator returned by [`Client::stream_updates`].
pub struct UpdateStream {
    client: Client,
    updates: UpdatesReceiver,
    buffer: VecDeque<(tl::enums::Update, State, PeerMap)>,
}

impl UpdateStream {
    /// Pops an update from the queue, waiting for an update to arrive first if the queue is empty.
    pub async fn next(&mut self) -> Result<Update, InvocationError> {
        let (update, state, peers) = self.next_raw().await?;
        Ok(Update::from_raw(&self.client, update, state, peers))
    }

    pub async fn next_raw(
        &mut self,
    ) -> Result<(tl::enums::Update, State, PeerMap), InvocationError> {
        loop {
            if let Some(update) = self.buffer.pop_front() {
                return Ok(update);
            }

            let (updates, users, chats) = self.updates.next().await?;
            let peer_map = self.client.build_peer_map(users, chats).await;
            self.buffer
                .extend(updates.into_iter().map(|(u, s)| (u, s, peer_map.handle())));
        }
    }

    /// Synchronize the updates state to the session.
    ///
    /// This is **not** automatically done on drop.
    pub async fn sync_update_state(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.updates.sync_update_state().await
    }
}

impl Client {
    /// Returns an asynchronous stream of processed updates.
    ///
    /// The updates are guaranteed to be in order, and any gaps will be resolved.\
    /// **Important** to note that for gaps to be resolved, the peers must have been
    /// persisted in the session cache beforehand (i.e. be retrievable with [`grammers_session::Session::peer`]).
    /// A good way to achieve this is to use [`Self::iter_dialogs`] at least once after login.
    ///
    /// The updates are wrapped in [`crate::update::Update`] to make them more convenient to use,
    /// but their raw type is still accessible to bridge any missing functionality.
    pub async fn stream_updates(
        &self,
        updates: mpsc::Receiver<UpdatesLike>,
        configuration: UpdatesConfiguration,
    ) -> Result<UpdateStream, Box<dyn std::error::Error + Send + Sync>> {
        let updates = UpdatesReceiver::create(
            self.0.handle.clone(),
            self.0.session.clone(),
            updates,
            configuration,
        )
        .await?;

        Ok(UpdateStream {
            client: self.clone(),
            updates,
            buffer: VecDeque::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use core::future::Future;

    use super::*;

    fn get_update_stream() -> UpdateStream {
        panic!()
    }

    #[test]
    fn ensure_next_update_future_impls_send() {
        if false {
            // We just want it to type-check, not actually run.
            fn typeck(_: impl Future + Send) {}
            typeck(get_update_stream().next());
        }
    }
}
