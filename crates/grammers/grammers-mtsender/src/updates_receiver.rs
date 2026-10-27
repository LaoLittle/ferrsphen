// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::collections::VecDeque;
use std::sync::Arc;

use grammers_session::storages::ErasedSession;
use grammers_session::types::{PeerId, PeerInfo, UpdateState, UpdatesState};
use grammers_session::updates::{
    MessageBoxes, PrematureEndReason, State, UpdateAndPeers, UpdatesLike,
};
use grammers_tl_types::{self as tl, Deserializable};
use tokio::sync::mpsc;
use tokio::time::timeout_at;
use tracing::{info, trace, warn};

use crate::{InvocationError, SenderPoolHandle, UpdatesConfiguration};

// See https://core.telegram.org/method/updates.getChannelDifference.
const BOT_CHANNEL_DIFF_LIMIT: i32 = 100000;
const USER_CHANNEL_DIFF_LIMIT: i32 = 100;

pub struct UpdatesReceiver {
    handle: SenderPoolHandle,
    session: Arc<ErasedSession>,
    message_box: MessageBoxes,
    updates: mpsc::Receiver<UpdatesLike>,
    buffer: VecDeque<UpdateAndPeers>,
    should_get_state: bool,
}

async fn prepare_channel_difference(
    mut request: tl::functions::updates::GetChannelDifference,
    session: Arc<ErasedSession>,
    message_box: &mut MessageBoxes,
) -> Result<
    Option<tl::functions::updates::GetChannelDifference>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let id = match &request.channel {
        tl::enums::InputChannel::Channel(channel) => PeerId::channel_unchecked(channel.channel_id),
        _ => unreachable!(),
    };

    if let Some(PeerInfo::Channel {
        id,
        auth: Some(auth),
        ..
    }) = session.peer(id).await?
    {
        request.channel = tl::enums::InputChannel::Channel(tl::types::InputChannel {
            channel_id: id,
            access_hash: auth.hash(),
        });
        request.limit = if session
            .peer(PeerId::self_user())
            .await?
            .map(|user| match user {
                PeerInfo::User { bot, .. } => bot.unwrap_or(false),
                _ => false,
            })
            .unwrap_or(false)
        {
            BOT_CHANNEL_DIFF_LIMIT
        } else {
            USER_CHANNEL_DIFF_LIMIT
        };
        trace!(?request, "requesting");
        Ok(Some(request))
    } else {
        warn!(
            channel_id = ?id,
            "cannot getChannelDifference as we're missing its hash",
        );
        message_box.end_channel_difference(PrematureEndReason::Banned);
        Ok(None)
    }
}

impl UpdatesReceiver {
    pub async fn create(
        handle: SenderPoolHandle,
        session: Arc<ErasedSession>,
        updates: mpsc::Receiver<UpdatesLike>,
        configuration: UpdatesConfiguration,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let message_box = if configuration.catch_up {
            MessageBoxes::load(session.updates_state().await?)
        } else {
            // If the user doesn't want to bother with catching up on previous update, start with
            // pristine state instead.
            MessageBoxes::new()
        };
        // Don't bother getting pristine update state if we're not logged in.
        let should_get_state =
            message_box.is_empty() && session.peer(PeerId::self_user()).await?.is_some();

        Ok(UpdatesReceiver {
            handle,
            session,
            message_box,
            updates,
            buffer: VecDeque::new(),
            should_get_state,
        })
    }

    /// Pops an updates from the queue, waiting for an updates to arrive first if the queue is empty.
    pub async fn next(
        &mut self,
    ) -> Result<
        (
            Vec<(tl::enums::Update, State)>,
            Vec<tl::enums::User>,
            Vec<tl::enums::Chat>,
        ),
        InvocationError,
    > {
        let session = self.session.clone();

        if self.should_get_state {
            self.should_get_state = false;
            match self.invoke(&tl::functions::updates::GetState {}).await {
                Ok(tl::enums::updates::State::State(state)) => {
                    session
                        .clone()
                        .set_update_state(UpdateState::All(UpdatesState {
                            pts: state.pts,
                            qts: state.qts,
                            date: state.date,
                            seq: state.seq,
                            channels: Vec::new(),
                        }))
                        .await?;
                    self.message_box.set_state(state);
                }
                Err(_err) => {
                    // The account may no longer actually be logged in, or it can rarely fail.
                    // `message_box` will try to correct its state as updates arrive.
                }
            }
        }

        loop {
            let (deadline, get_diff, get_channel_diff) = {
                if let Some(update) = self.buffer.pop_front() {
                    return Ok(update);
                }
                (
                    self.message_box.check_deadlines(), // first, as it might trigger differences
                    self.message_box.get_difference(),
                    match self.message_box.get_channel_difference() {
                        Some(gd) => {
                            prepare_channel_difference(gd, session.clone(), &mut self.message_box)
                                .await?
                        }
                        None => None,
                    },
                )
            };

            if let Some(request) = get_diff {
                let response = self.invoke(&request).await?;
                let (updates, users, chats) = self.message_box.apply_difference(response);
                self.extend_update_queue(updates, users, chats);
                continue;
            }

            if let Some(request) = get_channel_diff {
                let maybe_response = self.invoke(&request).await;

                let response = match maybe_response {
                    Ok(r) => r,
                    Err(e) if e.is("PERSISTENT_TIMESTAMP_OUTDATED") => {
                        // According to Telegram's docs:
                        // "Channel internal replication issues, try again later (treat this like an RPC_CALL_FAIL)."
                        // We can treat this as "empty difference" and not update the local pts.
                        // Then this same call will be retried when another gap is detected or timeout expires.
                        //
                        // Another option would be to literally treat this like an RPC_CALL_FAIL and retry after a few
                        // seconds, but if Telegram is having issues it's probably best to wait for it to send another
                        // update (hinting it may be okay now) and retry then.
                        //
                        // This is a bit hacky because MessageBox doesn't really have a way to "not update" the pts.
                        // Instead we manually extract the previously-known pts and use that.
                        warn!(
                            "Getting difference for channel updates caused PersistentTimestampOutdated; ending getting difference prematurely until server issues are resolved"
                        );
                        {
                            self.message_box
                                .end_channel_difference(PrematureEndReason::TemporaryServerIssues);
                        }
                        continue;
                    }
                    Err(e) if e.is("CHANNEL_PRIVATE") => {
                        info!(
                            ?request,
                            "Account is now banned so we can no longer fetch updates with request",
                        );
                        {
                            self.message_box
                                .end_channel_difference(PrematureEndReason::Banned);
                        }
                        continue;
                    }
                    Err(InvocationError::Rpc(rpc_error)) if rpc_error.code == 500 => {
                        warn!("Telegram is having internal issues: {:#?}", rpc_error);
                        {
                            self.message_box
                                .end_channel_difference(PrematureEndReason::TemporaryServerIssues);
                        }
                        continue;
                    }
                    Err(e) => return Err(e),
                };

                let (updates, users, chats) = self.message_box.apply_channel_difference(response);
                self.extend_update_queue(updates, users, chats);
                continue;
            }

            match timeout_at(deadline.into(), self.updates.recv()).await {
                Ok(Some(updates)) => self.process_socket_updates(updates).await,
                Ok(None) => break Err(InvocationError::Dropped("updates channel was closed")),
                Err(_) => {}
            }
        }
    }

    /// Synchronize the updates state to the session.
    ///
    /// This is **not** automatically done on drop.
    pub async fn sync_update_state(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.message_box.is_empty() {
            return Ok(());
        }
        self.session
            .clone()
            .set_update_state(UpdateState::All(self.message_box.session_state()))
            .await?;
        Ok(())
    }

    async fn invoke<R: tl::RemoteCall>(&self, request: &R) -> Result<R::Return, InvocationError> {
        let dc_id = self.session.clone().home_dc_id()?;
        self.handle
            .do_invoke_in_dc(dc_id, request.to_bytes())
            .await
            .and_then(|body| R::Return::from_bytes(&body).map_err(|e| e.into()))
    }

    async fn process_socket_updates(&mut self, updates: UpdatesLike) {
        if let Ok((updates, users, chats)) = self.message_box.process_updates(updates) {
            self.extend_update_queue(updates, users, chats);
        }
    }

    fn extend_update_queue(
        &mut self,
        updates: Vec<(tl::enums::Update, State)>,
        users: Vec<tl::enums::User>,
        chats: Vec<tl::enums::Chat>,
    ) {
        self.buffer.push_back((updates, users, chats));
    }
}
