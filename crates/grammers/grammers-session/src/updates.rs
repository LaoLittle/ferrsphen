// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Logic and types for [Working with Updates](https://core.telegram.org/api/updates).

use grammers_tl_types::{self as tl, Deserializable as _};

pub use crate::message_box::{
    Gap, MessageBox, MessageBoxes, PrematureEndReason, State, UpdateAndPeers, UpdatesLike,
};

impl UpdatesLike {
    /// Attempt to deserialize a buffer that may
    /// contain data pertaining to update handling.
    pub fn from_bytes(buf: &[u8]) -> tl::deserialize::Result<UpdatesLike> {
        match tl::enums::Updates::from_bytes(buf) {
            Ok(u) => Ok(UpdatesLike::Updates(u)),
            Err(e) => {
                if let Ok(tl::enums::messages::AffectedMessages::Messages(u)) =
                    tl::enums::messages::AffectedMessages::from_bytes(buf)
                {
                    Ok(UpdatesLike::AffectedMessages(u))
                } else if let Ok(tl::enums::messages::InvitedUsers::Users(u)) =
                    tl::enums::messages::InvitedUsers::from_bytes(buf)
                {
                    Ok(UpdatesLike::InvitedUsers(u))
                } else if let Ok(tl::enums::messages::ChatInviteJoinResult::Ok(u)) =
                    tl::enums::messages::ChatInviteJoinResult::from_bytes(buf)
                {
                    Ok(UpdatesLike::ChatInviteJoinResult(u))
                } else {
                    Err(e)
                }
            }
        }
    }
}
