// Copyright 2020 - developers of the `grammers` project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use grammers_mtsender::InvocationError;
use grammers_session::updates::State;
use grammers_tl_types as tl;

use crate::Client;
use crate::message::{InputMessage, Message};

#[derive(Clone, Debug)]
pub struct GuestChatQuery {
    pub message: Message,
    pub reference_messages: Vec<Message>,
    pub raw: tl::enums::Update,
    pub state: State,
}

impl GuestChatQuery {
    fn update(&self) -> &tl::types::UpdateBotGuestChatQuery {
        match &self.raw {
            tl::enums::Update::BotGuestChatQuery(update) => update,
            _ => unreachable!(),
        }
    }

    /// Query ID
    pub fn query_id(&self) -> i64 {
        self.update().query_id
    }

    pub async fn answer<T>(&self, result: T) -> Result<AnsweredMessage, InvocationError>
    where
        T: Into<tl::enums::InputBotInlineResult>,
    {
        let client = self.message.client.clone();
        let id = client
            .invoke(&tl::functions::messages::SetBotGuestChatResult {
                query_id: self.query_id(),
                result: result.into(),
            })
            .await?;
        Ok(AnsweredMessage { raw: id, client })
    }
}

pub struct AnsweredMessage {
    pub raw: tl::enums::InputBotInlineMessageId,
    pub(crate) client: Client,
}

impl AnsweredMessage {
    pub async fn edit(
        &self,
        input_message: impl Into<InputMessage>,
    ) -> Result<bool, InvocationError> {
        self.client
            .edit_inline_message(self.raw.clone(), input_message.into())
            .await
    }
}
