// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Types relating to users, groups and channels.
//!
//! Properties containing raw types are public and will either be called "raw" or prefixed with "raw_".\
//! Keep in mind that **these fields are not part of the semantic versioning guarantees**.

mod action;
pub(crate) mod chats;
mod dialog;
mod participant;
pub use action::ActionSender;
mod channel;
mod community;
mod group;
mod peer_map;
mod permissions;
mod user;

pub use channel::Channel;
pub use chats::{AdminRightsBuilder, BannedRightsBuilder};
pub use community::Community;
pub use dialog::Dialog;
use grammers_mtsender::InvocationError;
use grammers_session::types::{PeerAuth, PeerId, PeerInfo, PeerRef};
use grammers_tl_types as tl;
pub use group::Group;
pub use participant::{Participant, Role};
pub use peer_map::PeerMap;
pub use permissions::{Permissions, Restrictions};
pub use user::{Platform, RestrictionReason, User};

use crate::{Client, media::ChatPhoto};

/// Resolve a peer to its [`PeerRef`], using the cached `auth` if present or
/// falling back to the session cache. Shared by `Channel`, `Group` and `User`.
pub(crate) async fn to_ref(
    client: &Client,
    id: PeerId,
    auth: Option<PeerAuth>,
) -> Result<Option<PeerRef>, Box<dyn std::error::Error + Send + Sync>> {
    match auth {
        Some(auth) => Ok(Some(PeerRef { id, auth })),
        None => client.0.session.peer_ref(id).await,
    }
}

/// A user, group, or broadcast channel.
///
/// Peers represent places where you can initiate a message exchange with others.
/// After doing so, user accounts can retrieve them via their [`Dialog`] list.
///
/// * Private conversations with other people are treated as the peer of the user itself.
/// * Conversations in a group, whether it's private or public, are simply known as groups.
/// * Conversations where only administrators broadcast messages are known as channels.
#[derive(Clone, Debug)]
pub enum Peer {
    /// A [`User`].
    User(User),

    /// A [`Group`] chat.
    Group(Group),

    /// A broadcast [`Channel`].
    Channel(Channel),

    /// A [`Community`].
    Community(Community),
}

impl Peer {
    pub(crate) fn from_user(client: &Client, user: tl::enums::User) -> Self {
        Self::User(User::from_raw(client, user))
    }

    pub fn from_raw(client: &Client, chat: tl::enums::Chat) -> Self {
        use tl::enums::Chat as C;

        match chat {
            C::Empty(_) | C::Chat(_) | C::Forbidden(_) => {
                Self::Group(Group::from_raw(client, chat))
            }
            C::Channel(ref channel) => {
                if channel.broadcast {
                    Self::Channel(Channel::from_raw(client, chat))
                } else {
                    Self::Group(Group::from_raw(client, chat))
                }
            }
            C::ChannelForbidden(ref channel) => {
                if channel.broadcast {
                    Self::Channel(Channel::from_raw(client, chat))
                } else {
                    Self::Group(Group::from_raw(client, chat))
                }
            }
            C::CommunityForbidden(_) => Self::Community(Community::from_raw(client, chat)),
            C::Community(_) => Self::Community(Community::from_raw(client, chat)),
        }
    }

    /// Return the unique identifier for this peer.
    ///
    /// Every account will see the same identifier for the same peer.
    ///
    /// This identifier will never change. However, small group chats may be migrated to
    /// megagroups. If this happens, both the old small group chat and the new megagroup
    /// exist as separate chats with different identifiers, but they are linked with a
    /// property.
    pub fn id(&self) -> PeerId {
        match self {
            Self::User(user) => user.id(),
            Self::Group(group) => group.id(),
            Self::Channel(channel) => channel.id(),
            Self::Community(community) => community.id(),
        }
    }

    /// Non-min auth stored in the peer, if any.
    pub(crate) fn auth(&self) -> Option<PeerAuth> {
        match self {
            Self::User(user) => user.auth(),
            Self::Group(group) => group.auth(),
            Self::Channel(channel) => channel.auth(),
            Self::Community(community) => community.auth(),
        }
    }

    /// Convert the peer to its reference.
    ///
    /// This is only possible if the peer would be usable on all methods or if it is in the session cache.
    pub async fn to_ref(
        &self,
    ) -> Result<Option<PeerRef>, Box<dyn std::error::Error + Send + Sync>> {
        match self {
            Self::User(user) => user.to_ref().await,
            Self::Group(group) => group.to_ref().await,
            Self::Channel(channel) => channel.to_ref().await,
            Self::Community(community) => community.to_ref().await,
        }
    }

    /// Return the name of this peer.
    ///
    /// For private conversations (users), this is their first name. For groups and channels,
    /// this is their title.
    ///
    /// The name will be `None` if the peer is inaccessible or if the account was deleted. It may
    /// also be `None` if you received it previously.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::User(user) => user.first_name(),
            Self::Group(group) => group.title(),
            Self::Channel(channel) => Some(channel.title()),
            Self::Community(community) => Some(community.title()),
        }
    }

    /// Return the public @username of this peer, if any.
    ///
    /// The returned username does not contain the "@" prefix.
    ///
    /// Outside of the application, people may link to this user with one of Telegram's URLs, such
    /// as https://t.me/username.
    pub fn username(&self) -> Option<&str> {
        match self {
            Self::User(user) => user.username(),
            Self::Group(group) => group.username(),
            Self::Channel(channel) => channel.username(),
            Self::Community(_) => None,
        }
    }

    /// Return collectible usernames of this peer, if any.
    ///
    /// The returned usernames do not contain the "@" prefix.
    ///
    /// Outside of the application, people may link to this user with one of its username, such
    /// as https://t.me/username.
    pub fn usernames(&self) -> Vec<&str> {
        match self {
            Self::User(user) => user.usernames(),
            Self::Group(group) => group.usernames(),
            Self::Channel(channel) => channel.usernames(),
            Self::Community(_) => Vec::new(),
        }
    }

    // Return the profile picture or chat photo of this peer, if any.
    //
    // This does not fetch the photo, but it may query the session to fetch the peer information.
    pub async fn photo(&self, big: bool) -> Result<Option<ChatPhoto>, InvocationError> {
        let peer = match self.to_ref().await? {
            None => return Ok(None),
            Some(x) => x.into(),
        };
        let photo_id = match self {
            Self::User(user) => user.photo().map(|x| x.photo_id),
            Self::Group(group) => group.photo().map(|x| x.photo_id),
            Self::Channel(channel) => channel.photo().map(|x| x.photo_id),
            Self::Community(community) => community.photo().map(|x| x.photo_id),
        };
        Ok(photo_id.map(|photo_id| ChatPhoto {
            raw: tl::enums::InputFileLocation::InputPeerPhotoFileLocation(
                tl::types::InputPeerPhotoFileLocation {
                    big,
                    peer,
                    photo_id,
                },
            ),
        }))
    }
}

impl From<Peer> for PeerInfo {
    #[inline]
    fn from(peer: Peer) -> Self {
        <Self as From<&Peer>>::from(&peer)
    }
}
impl<'a> From<&'a Peer> for PeerInfo {
    fn from(peer: &'a Peer) -> Self {
        match peer {
            Peer::User(user) => <PeerInfo as From<&'a User>>::from(user),
            Peer::Group(group) => <PeerInfo as From<&'a Group>>::from(group),
            Peer::Channel(channel) => <PeerInfo as From<&'a Channel>>::from(channel),
            Peer::Community(community) => <PeerInfo as From<&'a Community>>::from(community),
        }
    }
}
