// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::fmt;

use grammers_session::types::{PeerAuth, PeerId, PeerInfo, PeerRef};
use grammers_tl_types as tl;

use crate::Client;

/// A community.
#[derive(Clone)]
pub struct Community {
    pub raw: tl::types::Community,
    pub(crate) client: Client,
}

impl fmt::Debug for Community {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.raw.fmt(f)
    }
}

impl Community {
    pub fn from_raw(client: &Client, chat: tl::enums::Chat) -> Self {
        use tl::enums::Chat as C;

        match chat {
            C::Empty(_) | C::Chat(_) | C::Forbidden(_) | C::Channel(_) | C::ChannelForbidden(_) => {
                panic!("cannot create from group chat or channel")
            }
            C::Community(community) => Self {
                raw: community,
                client: client.clone(),
            },
            C::CommunityForbidden(community) => Self {
                raw: tl::types::Community {
                    creator: false,
                    left: false,
                    min: false,
                    collapsed_in_dialogs: false,
                    id: community.id,
                    access_hash: community.access_hash,
                    title: community.title,
                    photo: tl::enums::ChatPhoto::Empty,
                    date: 0,
                    admin_rights: None,
                    default_banned_rights: None,
                },
                client: client.clone(),
            },
        }
    }

    /// Return the unique identifier for this community.
    pub fn id(&self) -> PeerId {
        PeerId::channel_unchecked(self.raw.id)
    }

    /// Non-min auth stored in the community, if any.
    pub(crate) fn auth(&self) -> Option<PeerAuth> {
        self.raw
            .access_hash
            .filter(|_| !self.raw.min)
            .map(PeerAuth::from_hash)
    }

    /// Convert the community to its reference.
    ///
    /// This is only possible if the peer would be usable on all methods or if it is in the session cache.
    pub async fn to_ref(
        &self,
    ) -> Result<Option<PeerRef>, Box<dyn std::error::Error + Send + Sync>> {
        super::to_ref(&self.client, self.id(), self.auth()).await
    }

    /// Return the title of this community.
    pub fn title(&self) -> &str {
        self.raw.title.as_str()
    }

    /// Return the photo of this community, if any.
    pub fn photo(&self) -> Option<&tl::types::ChatPhoto> {
        match &self.raw.photo {
            tl::enums::ChatPhoto::Empty => None,
            tl::enums::ChatPhoto::Photo(photo) => Some(photo),
        }
    }
}

impl From<Community> for PeerInfo {
    #[inline]
    fn from(community: Community) -> Self {
        <Self as From<&Community>>::from(&community)
    }
}
impl<'a> From<&'a Community> for PeerInfo {
    fn from(community: &'a Community) -> Self {
        <Self as From<&'a tl::types::Community>>::from(&community.raw)
    }
}
