// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::sync::Arc;

use crate::types::{DcOption, PeerId, PeerInfo, PeerRef, UpdateState, UpdatesState};
use crate::{BoxFuture, Session};

/// Erased session storage alias.
///
/// This is not a storage type by itself, but is what [`erase`]d storages are turned into.
///
/// Client implementations should generally not care about what the specific session storage is,
/// so erasing the session type is a good way to avoid infecting their type with more generics.
pub type ErasedSession = dyn Session<Error = Box<dyn std::error::Error + Send + Sync + 'static>>;

/// Erase a concrete session storage into one without a concrete error type.
pub fn erase<S>(session: Arc<S>) -> Arc<ErasedSession>
where
    S: Session + Sized,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    Arc::new(Eraser(session))
}

struct Eraser<S: Session>(Arc<S>);

impl<S> Session for Eraser<S>
where
    S: Session,
    S::Error: std::error::Error + Send + Sync,
{
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn home_dc_id(&self) -> Result<i32, Self::Error> {
        Arc::clone(&self.0).home_dc_id().map_err(|e| e.into())
    }

    fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, Result<(), Self::Error>> {
        Box::pin(async move {
            Arc::clone(&self.0)
                .set_home_dc_id(dc_id)
                .await
                .map_err(|e| e.into())
        })
    }

    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>, Self::Error> {
        Arc::clone(&self.0).dc_option(dc_id).map_err(|e| e.into())
    }

    fn set_dc_option(&self, dc_option: &DcOption) -> BoxFuture<'_, Result<(), Self::Error>> {
        let dc_option = dc_option.clone();
        Box::pin(async move {
            Arc::clone(&self.0)
                .set_dc_option(&dc_option)
                .await
                .map_err(|e| e.into())
        })
    }

    fn peer(&self, peer: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Self::Error>> {
        Box::pin(async move { Arc::clone(&self.0).peer(peer).await.map_err(|e| e.into()) })
    }

    fn peer_ref(&self, peer: PeerId) -> BoxFuture<'_, Result<Option<PeerRef>, Self::Error>> {
        Box::pin(async move {
            Arc::clone(&self.0)
                .peer_ref(peer)
                .await
                .map_err(|e| e.into())
        })
    }

    fn cache_peer(&self, peer: PeerInfo) -> BoxFuture<'_, Result<(), Self::Error>> {
        let peer = peer.clone();
        Box::pin(async move {
            Arc::clone(&self.0)
                .cache_peer(peer)
                .await
                .map_err(|e| e.into())
        })
    }

    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState, Self::Error>> {
        Box::pin(async {
            Arc::clone(&self.0)
                .updates_state()
                .await
                .map_err(|e| e.into())
        })
    }

    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, Result<(), Self::Error>> {
        Box::pin(async {
            Arc::clone(&self.0)
                .set_update_state(update)
                .await
                .map_err(|e| e.into())
        })
    }
}
