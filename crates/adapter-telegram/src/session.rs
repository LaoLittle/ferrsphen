use chat_adapter::{Error, Store};
use grammers_session::{
    BoxFuture, Session, SessionData,
    storages::MemorySession,
    types::{DcOption, PeerId, PeerInfo, UpdateState, UpdatesState},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize)]
struct Snapshot {
    home_dc: i32,
    dc_options: Vec<DcOption>,
    peers: Vec<PeerInfo>,
    updates: UpdatesState,
}
pub struct StoredSession {
    memory: MemorySession,
    store: Arc<dyn Store>,
    key: String,
    writer: Mutex<()>,
}
fn failure(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}
impl StoredSession {
    pub async fn open(store: Arc<dyn Store>, key: String) -> chat_adapter::Result<Self> {
        let data = if let Some(value) = store.get(key.clone()).await? {
            let snapshot: Snapshot = serde_json::from_value(value).map_err(failure)?;
            SessionData {
                home_dc: snapshot.home_dc,
                dc_options: snapshot
                    .dc_options
                    .into_iter()
                    .map(|dc| (dc.id, dc))
                    .collect(),
                peer_infos: snapshot
                    .peers
                    .into_iter()
                    .map(|peer| (peer.id(), peer))
                    .collect(),
                updates_state: snapshot.updates,
            }
        } else {
            SessionData::default()
        };
        Ok(Self {
            memory: data.into(),
            store,
            key,
            writer: Mutex::new(()),
        })
    }
    pub async fn persist(&self) -> chat_adapter::Result<()> {
        let _guard = self.writer.lock().await;
        let data = self.memory.snapshot().map_err(failure)?;
        self.store
            .put(
                self.key.clone(),
                serde_json::to_value(Snapshot {
                    home_dc: data.home_dc,
                    dc_options: data.dc_options.into_values().collect(),
                    peers: data.peer_infos.into_values().collect(),
                    updates: data.updates_state,
                })
                .map_err(failure)?,
            )
            .await
    }
}
impl Session for StoredSession {
    type Error = Error;
    fn home_dc_id(&self) -> Result<i32, Error> {
        self.memory.home_dc_id().map_err(failure)
    }
    fn dc_option(&self, id: i32) -> Result<Option<DcOption>, Error> {
        self.memory.dc_option(id).map_err(failure)
    }
    fn set_home_dc_id(&self, id: i32) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            self.memory.set_home_dc_id(id).await.map_err(failure)?;
            self.persist().await
        })
    }
    fn set_dc_option(&self, dc: &DcOption) -> BoxFuture<'_, Result<(), Error>> {
        let dc = dc.clone();
        Box::pin(async move {
            self.memory.set_dc_option(&dc).await.map_err(failure)?;
            self.persist().await
        })
    }
    fn peer(&self, id: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Error>> {
        Box::pin(async move { self.memory.peer(id).await.map_err(failure) })
    }
    fn cache_peer(&self, peer: PeerInfo) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            self.memory.cache_peer(peer).await.map_err(failure)?;
            self.persist().await
        })
    }
    fn cache_peers(&self, peers: Vec<PeerInfo>) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            for peer in peers {
                self.memory.cache_peer(peer).await.map_err(failure)?;
            }
            self.persist().await
        })
    }
    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState, Error>> {
        Box::pin(async move { self.memory.updates_state().await.map_err(failure) })
    }
    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            self.memory
                .set_update_state(update)
                .await
                .map_err(failure)?;
            self.persist().await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn keys_peers_and_update_positions_survive_store_reopen() {
        let store: Arc<dyn Store> = Arc::new(chat_adapter::mock::MemoryStore::default());
        let session = StoredSession::open(store.clone(), "one".into())
            .await
            .unwrap();
        let mut dc = session.dc_option(2).unwrap().unwrap();
        dc.auth_key = Some([37; 256]);
        session.set_dc_option(&dc).await.unwrap();
        session.set_home_dc_id(2).await.unwrap();
        let peer = PeerInfo::User {
            id: 42,
            auth: Some(grammers_session::types::PeerAuth::default()),
            bot: Some(false),
            is_self: Some(true),
        };
        session.cache_peer(peer.clone()).await.unwrap();
        session
            .set_update_state(UpdateState::Primary {
                pts: 12,
                date: 34,
                seq: 56,
            })
            .await
            .unwrap();
        drop(session);
        let reopened = StoredSession::open(store.clone(), "one".into())
            .await
            .unwrap();
        assert_eq!(
            reopened.dc_option(2).unwrap().unwrap().auth_key,
            Some([37; 256])
        );
        assert_eq!(
            reopened.peer(PeerId::user_unchecked(42)).await.unwrap(),
            Some(peer)
        );
        assert_eq!(reopened.updates_state().await.unwrap().pts, 12);
        let other = StoredSession::open(store, "two".into()).await.unwrap();
        assert!(other.dc_option(2).unwrap().unwrap().auth_key.is_none());
        assert!(
            other
                .peer(PeerId::user_unchecked(42))
                .await
                .unwrap()
                .is_none()
        );
    }
}
