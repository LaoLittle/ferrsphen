use chat_adapter::*;
use matrix_sdk::{Room, ruma};
use serde_json::{Value, json};
pub fn avatar(uri: &str) -> Media {
    Media {
        id: uri.into(),
        mime: None,
        name: None,
        width: None,
        height: None,
        duration_ms: None,
        byte_length: None,
        data: json!({"url":uri}),
    }
}
pub async fn conversation(account: &AccountId, room: &Room) -> Result<Conversation> {
    Ok(Conversation {
        id: account.chat(room.room_id().as_str()),
        kind: if room.is_direct().await.map_err(super::failure)? {
            ChatKind::Direct
        } else {
            ChatKind::Group
        },
        title: room
            .display_name()
            .await
            .map_err(super::failure)?
            .to_string(),
        avatar: room.avatar_url().map(|uri| Box::new(avatar(uri.as_str()))),
        unread: room
            .unread_notification_counts()
            .notification_count
            .min(u64::from(u32::MAX)) as u32,
        last_activity_ms: room
            .latest_event_timestamp()
            .map_or(0, |time| u64::from(time.0).min(i64::MAX as u64) as i64),
    })
}
pub async fn event(account: &AccountId, room: &Room, value: Value) -> Result<Option<Event>> {
    let sender = value
        .get("sender")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let uid = ruma::UserId::parse(sender).ok();
    let member = if let Some(uid) = &uid {
        room.get_member_no_sync(uid).await.map_err(super::failure)?
    } else {
        None
    };
    let user = User {
        id: sender.into(),
        name: member
            .as_ref()
            .map(|m| m.name().to_owned())
            .or_else(|| uid.as_ref().map(|u| u.localpart().to_owned()))
            .unwrap_or_else(|| "Matrix".into()),
        avatar: member
            .as_ref()
            .and_then(|m| m.avatar_url())
            .map(|url| Box::new(avatar(url.as_str()))),
    };
    Ok(map(
        account,
        room.room_id().as_str(),
        room.client().user_id().map(|id| id.as_str()),
        value,
        user,
    ))
}
pub fn map(
    account: &AccountId,
    room: &str,
    own: Option<&str>,
    value: Value,
    sender: User,
) -> Option<Event> {
    let chat = account.chat(room);
    let kind = value.get("type")?.as_str()?;
    let id = value.get("event_id")?.as_str()?.to_owned();
    let content = value.get("content").cloned().unwrap_or(json!({}));
    if kind == "m.room.redaction" {
        let target = value
            .get("redacts")
            .or_else(|| content.get("redacts"))?
            .as_str()?;
        return Some(Event::Deleted(vec![MessageId {
            chat,
            id: target.into(),
        }]));
    }
    let deleted = value.pointer("/unsigned/redacted_because").is_some();
    let mut parts = vec![];
    let extension = |kind: &str, layout: Layout, text: String| {
        Content::Extension(ExtensionContent {
            kind: kind.into(),
            version: 1,
            data: value.clone(),
            layout,
            display: vec![DisplayPart::Text(text)],
        })
    };
    match kind {
        "m.room.message" => {
            let body = content
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let msgtype = content
                .get("msgtype")
                .and_then(Value::as_str)
                .unwrap_or("m.text");
            match msgtype {
                "m.text" | "m.notice" | "m.emote" => {
                    if !body.is_empty() {
                        parts.push(Content::Text(body.into()));
                    }
                }
                "m.image" | "m.video" | "m.file" | "m.audio" => {
                    let info = content.get("info");
                    let number =
                        |key: &str| info.and_then(|info| info.get(key)).and_then(Value::as_u64);
                    let data = if let Some(file) = content.get("file") {
                        json!({"file":file})
                    } else {
                        json!({"url":content.get("url")?})
                    };
                    let reference = Media {
                        id: format!("{room}:{id}"),
                        mime: info
                            .and_then(|info| info.get("mimetype"))
                            .and_then(Value::as_str)
                            .map(String::from),
                        name: Some(body.into()),
                        width: number("w").and_then(|v| v.try_into().ok()),
                        height: number("h").and_then(|v| v.try_into().ok()),
                        duration_ms: number("duration"),
                        byte_length: number("size"),
                        data,
                    };
                    parts.push(match msgtype {
                        "m.image" => Content::Image(reference),
                        "m.video" => Content::Video(reference),
                        "m.audio" if content.get("org.matrix.msc3245.voice").is_some() => {
                            Content::Voice(reference)
                        }
                        "m.audio" => Content::Audio(reference),
                        _ => Content::File(reference),
                    });
                }
                _ => parts.push(extension("matrix.message", Layout::Bubble, body.into())),
            }
        }
        "m.sticker" => {
            let mut image = avatar(content.get("url")?.as_str()?);
            image.name = content
                .get("body")
                .and_then(Value::as_str)
                .map(String::from);
            parts.push(Content::Sticker(image));
        }
        "m.room.encrypted" => parts.push(extension(
            "matrix.encrypted",
            Layout::Bubble,
            "Encrypted message".into(),
        )),
        "m.reaction" => {
            let key = content.pointer("/m.relates_to/key")?.as_str()?;
            parts.push(extension(
                "matrix.reaction",
                Layout::Notice,
                format!("{} {key}", sender.name),
            ));
        }
        "m.room.member" => {
            let membership = content.get("membership").and_then(Value::as_str)?;
            let previous = value
                .pointer("/unsigned/prev_content/membership")
                .and_then(Value::as_str);
            if previous == Some(membership) {
                return None;
            }
            let text = match membership {
                "join" => "Joined the room",
                "leave" => "Left the room",
                "invite" => "Invited a member",
                "ban" => "Banned a member",
                _ => return None,
            };
            parts.push(extension(
                "matrix.membership",
                Layout::Notice,
                format!("{}: {text}", sender.name),
            ));
        }
        "m.room.name" => parts.push(extension(
            "matrix.room_name",
            Layout::Notice,
            content
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        )),
        "m.room.topic" => parts.push(extension(
            "matrix.topic",
            Layout::Notice,
            content
                .get("topic")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        )),
        _ => return None,
    }
    let reply = content
        .pointer("/m.relates_to/m.in_reply_to/event_id")
        .and_then(Value::as_str)
        .map(|id| {
            Box::new(Reply {
                target: MessageId {
                    chat: chat.clone(),
                    id: id.into(),
                },
                sender: User {
                    id: String::new(),
                    name: String::new(),
                    avatar: None,
                },
                preview: vec![],
            })
        });
    Some(Event::Message(Message {
        id: MessageId { chat, id },
        client_id: value
            .pointer("/unsigned/transaction_id")
            .and_then(Value::as_str)
            .map(String::from),
        outgoing: own == Some(sender.id.as_str()),
        sender,
        time_ms: value
            .get("origin_server_ts")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        content: parts,
        reply,
        deleted,
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matrix_metadata_is_not_a_message_and_encrypted_file_references_are_preserved() {
        let account = AccountId {
            adapter: "matrix".into(),
            id: "one".into(),
        };
        let user = User {
            id: "@a:s".into(),
            name: "Alice".into(),
            avatar: None,
        };
        assert!(map(&account,"!r:s",None,json!({"type":"m.room.power_levels","sender":"@a:s","event_id":"$meta","content":{}}),user.clone()).is_none());
        let event=map(&account,"!r:s",None,json!({"type":"m.room.message","sender":"@a:s","event_id":"$image","origin_server_ts":10,"content":{"msgtype":"m.image","body":"x.png","file":{"url":"mxc://s/file","key":{"k":"secret"}}}}),user).unwrap();
        let Event::Message(message) = event else {
            panic!()
        };
        let Content::Image(image) = &message.content[0] else {
            panic!()
        };
        assert_eq!(image.data["file"]["key"]["k"], "secret");
        assert_eq!(message.id.chat.account, account);
    }
}
