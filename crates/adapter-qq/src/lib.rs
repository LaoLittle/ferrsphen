//! QQ adapter and lossless mapping.
use atri_qq::{
    ServiceClient,
    admin::Admin,
    contacts::Operations,
    media::{Media as QqMedia, MediaReference},
    messages::{Conversation as QqChat, MessageElement, Messaging, PushEvent, RoamDirection},
};
use chat_adapter::*;
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, broadcast};
fn invalid(error: impl std::fmt::Display) -> Error {
    Error::Invalid(error.to_string())
}
fn remote(error: atri_qq::Error) -> Error {
    match error {
        atri_qq::Error::Server { code, message } => Error::Remote {
            code: code.to_string(),
            message,
            retry_after_seconds: None,
        },
        error => Error::Transport(error.to_string()),
    }
}
pub fn chat_id(account: &AccountId, chat: &QqChat) -> ChatId {
    account.chat(serde_json::to_string(chat).expect("QQ conversation"))
}
fn native_chat(account: &AccountId, chat: &ChatId) -> Result<QqChat> {
    account.require_chat(chat)?;
    serde_json::from_str(&chat.id).map_err(invalid)
}
fn media(reference: &MediaReference) -> Media {
    Media {
        id: format!(
            "qq:{}:{}",
            hex::encode(&reference.md5),
            reference
                .uuid
                .as_deref()
                .or(reference.file_id.as_deref())
                .unwrap_or(&reference.name)
        ),
        mime: None,
        name: (!reference.name.is_empty()).then(|| reference.name.clone()),
        width: (reference.width != 0).then_some(reference.width),
        height: (reference.height != 0).then_some(reference.height),
        duration_ms: (reference.duration != 0).then_some(u64::from(reference.duration) * 1000),
        byte_length: (reference.size != 0).then_some(reference.size),
        data: serde_json::to_value(reference).expect("QQ media"),
    }
}
pub fn normalize_message(account: &AccountId, message: &atri_qq::messages::Message) -> Message {
    let chat = chat_id(account, &message.conversation);
    let content = message
        .elements
        .iter()
        .filter_map(|part| match part {
            MessageElement::Text(text) => Some(Content::Text(text.clone())),
            MessageElement::Mention(mention) => {
                Some(Content::Text(format!("@{}", mention.display)))
            }
            MessageElement::Reply(_) => None,
            MessageElement::Image(reference) => Some(Content::Image(media(reference))),
            MessageElement::Video(reference) => Some(Content::Video(media(reference))),
            MessageElement::Voice(reference) => Some(Content::Voice(media(reference))),
            MessageElement::File(reference) => Some(Content::File(media(reference))),
            MessageElement::Opaque(opaque)
                if atri_qq::messages::is_protocol_metadata(opaque.scope, opaque.field) =>
            {
                None
            }
            part => Some(Content::Extension(ExtensionContent {
                kind: match part {
                    MessageElement::LightApp(_) => "qq.light_app",
                    MessageElement::Forward(_) => "qq.forward",
                    MessageElement::GroupFile(_) => "qq.group_file",
                    _ => "qq.element",
                }
                .into(),
                version: 1,
                data: serde_json::to_value(part).expect("QQ element"),
                layout: Layout::Card,
                display: vec![DisplayPart::Text(match part {
                    MessageElement::GroupFile(file) => file.name.clone(),
                    MessageElement::Forward(_) => "Chat history".into(),
                    MessageElement::LightApp(json) => {
                        serde_json::from_str::<serde_json::Value>(json)
                            .ok()
                            .and_then(|value| {
                                value
                                    .get("prompt")
                                    .and_then(|v| v.as_str())
                                    .map(String::from)
                            })
                            .unwrap_or_else(|| "QQ card".into())
                    }
                    _ => "QQ message".into(),
                })],
            })),
        })
        .collect();
    let id = if message.sequence != 0 {
        message.sequence.to_string()
    } else {
        format!("random:{}", message.random)
    };
    Message {
        id: MessageId { chat, id },
        client_id: (message.is_self && message.random != 0).then(|| message.random.to_string()),
        sender: User {
            id: if message.sender_uid.is_empty() {
                message.sender_uin.to_string()
            } else {
                message.sender_uid.clone()
            },
            name: message.sender_name.clone(),
            avatar: None,
        },
        time_ms: message.time.saturating_mul(1000),
        outgoing: message.is_self,
        content,
        reply: None,
        deleted: message.is_recalled(),
    }
}
/// Declarative representation of a platform-specific notification.
pub fn poke_content(
    event: &PushEvent,
    actor: User,
    target: User,
    fallback_action: &str,
) -> Option<Content> {
    let PushEvent::Poke { parameters, .. } = event else {
        return None;
    };
    let action = parameters
        .get("action_str")
        .filter(|text| !text.trim().is_empty())
        .map_or(fallback_action, String::as_str);
    let suffix = parameters.get("suffix_str").map_or("", String::as_str);
    Some(Content::Extension(ExtensionContent {
        kind: "qq.poke".into(),
        version: 1,
        data: serde_json::to_value(event).expect("QQ poke"),
        layout: Layout::Notice,
        display: vec![
            DisplayPart::User(actor),
            DisplayPart::Text(format!(" {action} ")),
            DisplayPart::User(target),
            DisplayPart::Text(suffix.into()),
        ],
    }))
}
async fn normalized_poke(
    account: &AccountId,
    event: &PushEvent,
    operations: &Operations,
    own: u64,
) -> Result<Option<Message>> {
    let PushEvent::Poke {
        group_uin,
        actor_uin,
        target_uin,
        ..
    } = event
    else {
        return Ok(None);
    };
    let friends = operations.fetch_friends(false).await.map_err(remote)?;
    let members = if let Some(group) = group_uin {
        operations
            .fetch_members(*group, false)
            .await
            .map_err(remote)?
    } else {
        vec![]
    };
    let name = |uin: Option<u64>| {
        let name = uin
            .and_then(|uin| friends.iter().find(|friend| friend.uin == uin))
            .map(|friend| {
                if friend.remark.is_empty() {
                    friend.nickname.clone()
                } else {
                    friend.remark.clone()
                }
            })
            .or_else(|| {
                uin.and_then(|uin| members.iter().find(|member| member.uin == uin))
                    .map(|member| {
                        member
                            .card
                            .as_ref()
                            .filter(|name| !name.is_empty())
                            .cloned()
                            .unwrap_or_else(|| member.nickname.clone())
                    })
            })
            .unwrap_or_else(|| "QQ".into());
        User {
            id: uin.unwrap_or_default().to_string(),
            name,
            avatar: None,
        }
    };
    let native = if let Some(group) = group_uin {
        QqChat::Group { uin: *group }
    } else {
        let Some(peer) = [*actor_uin, *target_uin]
            .into_iter()
            .flatten()
            .find(|uin| *uin != own && *uin != 0)
        else {
            return Ok(None);
        };
        let uid = operations.resolve_uid(peer, None).await.map_err(remote)?;
        QqChat::Friend { uin: peer, uid }
    };
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let content = poke_content(event, name(*actor_uin), name(*target_uin), "poked").unwrap();
    Ok(Some(Message {
        id: MessageId {
            chat: chat_id(account, &native),
            id: format!("poke:{}", time.as_nanos()),
        },
        client_id: None,
        sender: name(*actor_uin),
        time_ms: time.as_millis().min(i64::MAX as u128) as i64,
        outgoing: *actor_uin == Some(own),
        content: vec![content],
        reply: None,
        deleted: false,
    }))
}
#[derive(serde::Serialize, serde::Deserialize)]
enum Cursor {
    Group(u64),
    Direct { time: u32, random: u32 },
}
pub struct QqAdapter {
    account: AccountId,
    client: Arc<dyn ServiceClient>,
    messaging: Arc<Messaging>,
    operations: Operations,
    media: Arc<QqMedia>,
    events: broadcast::Sender<Event>,
    relay: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl QqAdapter {
    pub fn attach(account: impl Into<String>, client: Arc<dyn ServiceClient>) -> Self {
        let media = Arc::new(QqMedia::new(client.clone()));
        Self {
            account: AccountId {
                adapter: "qq".into(),
                id: account.into(),
            },
            operations: Operations::new(client.clone()),
            messaging: Arc::new(Messaging::new(client.clone(), media.clone())),
            client,
            media,
            events: broadcast::channel(1024).0,
            relay: Mutex::new(None),
        }
    }
    async fn route(&self, chat: &ChatId) -> Result<QqChat> {
        match native_chat(&self.account, chat)? {
            QqChat::Friend { uin, uid } if uid.is_empty() => Ok(QqChat::Friend {
                uin,
                uid: self
                    .operations
                    .resolve_uid(uin, None)
                    .await
                    .map_err(remote)?,
            }),
            chat => Ok(chat),
        }
    }
}
impl Adapter for QqAdapter {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            id: "qq".into(),
            name: "QQ".into(),
            capabilities: BTreeSet::from_iter(
                [
                    "core.text",
                    "core.history",
                    "core.media.download",
                    "qq.poke",
                    "session.attach",
                ]
                .map(String::from),
            ),
            actions: vec![Action {
                id: "qq.poke".into(),
                label: "戳一戳".into(),
                scope: Scope::User,
                fields: vec![],
            }],
        }
    }
    fn account(&self) -> AccountId {
        self.account.clone()
    }
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
    fn authenticate(&self, _: AuthInput) -> Task<'_, AuthState> {
        Box::pin(async move {
            let state = self.client.snapshot().await;
            if !state.online {
                return Err(Error::Unauthorized);
            }
            let mut relay = self.relay.lock().await;
            if relay.is_none() {
                let mut packets = self.client.subscribe();
                let messaging = self.messaging.clone();
                let account = self.account.clone();
                let events = self.events.clone();
                let operations = Operations::new(self.client.clone());
                let own = state.keystore.uin;
                *relay = Some(tokio::spawn(async move {
                    loop {
                        match packets.recv().await {
                            Ok(packet) => {
                                for event in messaging.decode_push(packet).await {
                                    match event {
                                        PushEvent::Message { message } => {
                                            let _ = events.send(Event::Message(normalize_message(
                                                &account, &message,
                                            )));
                                        }
                                        PushEvent::Recall {
                                            conversation,
                                            sequence,
                                            ..
                                        } => {
                                            let _ = events.send(Event::Deleted(vec![MessageId {
                                                chat: chat_id(&account, &conversation),
                                                id: sequence.to_string(),
                                            }]));
                                        }
                                        event @ PushEvent::Poke { .. } => match normalized_poke(
                                            &account,
                                            &event,
                                            &operations,
                                            own,
                                        )
                                        .await
                                        {
                                            Ok(Some(message)) => {
                                                let _ = events.send(Event::Message(message));
                                            }
                                            Ok(None) => {}
                                            Err(error) => {
                                                let _ = events.send(Event::Error(error));
                                            }
                                        },
                                        PushEvent::Membership { .. } => {
                                            let _ = events.send(Event::ResyncRequired);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                let _ = events.send(Event::ResyncRequired);
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }));
            }
            let me = self
                .operations
                .fetch_stranger_uin(state.keystore.uin)
                .await
                .map_err(remote)?;
            Ok(AuthState::Ready {
                account: self.account.clone(),
                user: User {
                    id: state.keystore.uin.to_string(),
                    name: me.nickname,
                    avatar: None,
                },
            })
        })
    }
    fn conversations(&self, page: PageRequest) -> Task<'_, Page<Conversation>> {
        Box::pin(async move {
            page.validate()?;
            let (friends, groups) = tokio::try_join!(
                self.operations.fetch_friends(false),
                self.operations.fetch_groups(false)
            )
            .map_err(remote)?;
            let mut items: Vec<_> = friends
                .into_iter()
                .map(|friend| Conversation {
                    id: chat_id(
                        &self.account,
                        &QqChat::Friend {
                            uin: friend.uin,
                            uid: friend.uid,
                        },
                    ),
                    kind: ChatKind::Direct,
                    title: if friend.remark.is_empty() {
                        friend.nickname
                    } else {
                        friend.remark
                    },
                    avatar: None,
                    unread: 0,
                    last_activity_ms: 0,
                })
                .chain(groups.into_iter().map(|group| Conversation {
                    id: chat_id(&self.account, &QqChat::Group { uin: group.uin }),
                    kind: ChatKind::Group,
                    title: group.remark.filter(|s| !s.is_empty()).unwrap_or(group.name),
                    avatar: None,
                    unread: 0,
                    last_activity_ms: group.last_speak_time.min(i64::MAX as u64) as i64 * 1000,
                }))
                .collect();
            items.sort_by(|a, b| a.id.cmp(&b.id));
            let start = page
                .cursor
                .map(|cursor| cursor.parse::<usize>().map_err(invalid))
                .transpose()?
                .unwrap_or(0);
            let total = items.len();
            let end = start.saturating_add(page.limit as usize).min(total);
            let next = (end < total).then(|| end.to_string());
            Ok(Page {
                items: items
                    .into_iter()
                    .skip(start)
                    .take(page.limit as usize)
                    .collect(),
                next,
            })
        })
    }
    fn history(&self, chat: ChatId, page: PageRequest) -> Task<'_, Page<Message>> {
        Box::pin(async move {
            page.validate()?;
            let native = self.route(&chat).await?;
            let cursor: Option<Cursor> = page
                .cursor
                .map(|cursor| serde_json::from_str(&cursor).map_err(invalid))
                .transpose()?;
            let (messages, next) = match native {
                QqChat::Group { uin } => {
                    let end = match cursor {
                        Some(Cursor::Group(end)) => end,
                        Some(_) => return Err(invalid("Wrong history cursor")),
                        None => u64::from(
                            self.operations
                                .fetch_groups(false)
                                .await
                                .map_err(remote)?
                                .into_iter()
                                .find(|g| g.uin == uin)
                                .ok_or_else(|| invalid("Unknown group"))?
                                .latest_sequence,
                        ),
                    };
                    if end == 0 {
                        return Ok(Page {
                            items: vec![],
                            next: None,
                        });
                    }
                    let start = end.saturating_sub(u64::from(page.limit) - 1).max(1);
                    let messages = match self.messaging.history_group(uin, start, end).await {
                        Ok(messages) => messages,
                        Err(atri_qq::Error::Server { code: 4, .. }) => {
                            return Ok(Page {
                                items: vec![],
                                next: None,
                            });
                        }
                        Err(error) => return Err(remote(error)),
                    };
                    let next =
                        (!messages.is_empty() && start > 1).then_some(Cursor::Group(start - 1));
                    (messages, next)
                }
                QqChat::Friend { uid, .. } => {
                    let (time, random) = match cursor {
                        Some(Cursor::Direct { time, random }) => (time, random),
                        Some(_) => return Err(invalid("Wrong history cursor")),
                        None => (u32::MAX, 0),
                    };
                    let page = self
                        .messaging
                        .roam(
                            &uid,
                            time,
                            random,
                            page.limit.min(30),
                            RoamDirection::Upwards,
                        )
                        .await
                        .map_err(remote)?;
                    let next = (!page.is_complete && !page.messages.is_empty()).then_some(
                        Cursor::Direct {
                            time: page.time,
                            random: page.random,
                        },
                    );
                    (page.messages, next)
                }
                _ => return Err(Error::Unsupported("Temporary QQ history".into())),
            };
            Ok(Page {
                items: messages
                    .iter()
                    .map(|message| normalize_message(&self.account, message))
                    .collect(),
                next: next.map(|cursor| serde_json::to_string(&cursor).unwrap()),
            })
        })
    }
    fn send(&self, request: SendRequest) -> Task<'_, SendOutcome> {
        Box::pin(async move {
            let native = self.route(&request.chat).await?;
            if request.reply_to.is_some() {
                return Err(Error::Unsupported(
                    "QQ replies require a native reply reference".into(),
                ));
            }
            let elements: Vec<_> = request
                .content
                .into_iter()
                .map(|part| match part {
                    Content::Text(text) => Ok(MessageElement::Text(text)),
                    _ => Err(Error::Unsupported("QQ non-text send".into())),
                })
                .collect::<Result<_>>()?;
            let bytes = sha1_smol::Sha1::from(request.client_id.as_bytes())
                .digest()
                .bytes();
            let nonce = (u32::from_le_bytes(bytes[..4].try_into().unwrap()) & 0x7fff_ffff).max(1);
            let message = self
                .messaging
                .send_with_nonce(native, elements, nonce)
                .await
                .map_err(remote)?;
            let mut mapped = normalize_message(&self.account, &message);
            mapped.client_id = Some(request.client_id);
            Ok(SendOutcome::confirmed(mapped))
        })
    }
    fn download(&self, media: Media, path: PathBuf) -> Task<'_, PathBuf> {
        Box::pin(async move {
            let reference: MediaReference = serde_json::from_value(media.data).map_err(invalid)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(invalid)?;
            }
            self.media
                .download(&reference, &path)
                .await
                .map_err(remote)?;
            Ok(path)
        })
    }
    fn execute(&self, request: ExtensionRequest) -> Task<'_, serde_json::Value> {
        Box::pin(async move {
            if request.action != "qq.poke" {
                return Err(Error::Unsupported(request.action));
            }
            let user = request.user.ok_or_else(|| invalid("Select a user"))?;
            let target = match user.parse::<u64>() {
                Ok(uin) => uin,
                Err(_) => self.operations.resolve_uin(&user).await.map_err(remote)?,
            };
            let native = self
                .route(
                    &request
                        .chat
                        .ok_or_else(|| invalid("Select a conversation"))?,
                )
                .await?;
            let admin = Admin::new(self.client.clone());
            match native {
                QqChat::Group { uin } => {
                    admin.send_group_nudge(uin, target).await.map_err(remote)?
                }
                QqChat::Friend { uin, .. } => admin
                    .send_friend_nudge(uin, Some(target))
                    .await
                    .map_err(remote)?,
                _ => return Err(Error::Unsupported("Temporary QQ poke".into())),
            }
            Ok(serde_json::Value::Null)
        })
    }
    fn close(&self) -> Task<'_, ()> {
        Box::pin(async move {
            if let Some(task) = self.relay.lock().await.take() {
                task.abort();
                let _ = task.await;
            }
            Ok(())
        })
    }
}
impl Drop for QqAdapter {
    fn drop(&mut self) {
        if let Ok(mut relay) = self.relay.try_lock()
            && let Some(task) = relay.take()
        {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn poke_preserves_native_data_and_renders_with_only_the_common_contract() {
        let event = PushEvent::Poke {
            group_uin: Some(42),
            actor_uin: Some(1),
            target_uin: Some(2),
            parameters: std::collections::BTreeMap::from([("suffix_str".into(), " 👋".into())]),
            raw: vec![1, 2, 3],
        };
        let person = |id: &str, name: &str| User {
            id: id.into(),
            name: name.into(),
            avatar: None,
        };
        let content =
            poke_content(&event, person("1", "Alice"), person("2", "Bob"), "poked").unwrap();
        assert_eq!(content.text_preview(), "Alice poked Bob 👋");
        let Content::Extension(extension) = content else {
            panic!()
        };
        assert_eq!(extension.kind, "qq.poke");
        assert_eq!(extension.layout, Layout::Notice);
        let restored: PushEvent = serde_json::from_value(extension.data).unwrap();
        assert!(matches!(restored,PushEvent::Poke {raw,..} if raw==[1,2,3]));
    }
}
