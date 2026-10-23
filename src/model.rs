use atri_qq::{
    UserInfo,
    contacts::{Friend, Group, GroupAnnouncement, GroupMember},
    messages::{Conversation, Message, MessageElement, Reply},
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub fn conversation_key(conversation: &Conversation) -> String {
    match conversation {
        Conversation::Friend { uin, uid } => {
            if uid.is_empty() {
                format!("dm:uin:{uin}")
            } else {
                format!("dm:uid:{uid}")
            }
        }
        Conversation::Group { uin } => format!("group:{uin}"),
        Conversation::Temporary { uin, uid, source } => format!("temp:{source}:{uin}:{uid}"),
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Chat {
    pub conversation: Conversation,
    pub title: String,
    pub draft: String,
    pub unread: usize,
}
#[derive(Clone, Debug)]
pub enum Cursor {
    Group(u64),
    Direct { time: u32, random: u32 },
}
#[derive(Default)]
pub struct History {
    pub loading: bool,
    pub loaded: bool,
    pub cursor: Option<Cursor>,
    pub error: Option<String>,
}
#[derive(Default)]
pub struct Model {
    pub profile: Option<UserInfo>,
    pub chats: Vec<Chat>,
    pub selected: Option<String>,
    pub friends: Vec<Friend>,
    pub groups: Vec<Group>,
    pub members: HashMap<u64, Vec<GroupMember>>,
    pub resolved_uins: HashMap<String, u64>,
    pub announcements: HashMap<u64, Result<Vec<GroupAnnouncement>, String>>,
    pub messages: HashMap<String, Vec<Message>>,
    pub history: HashMap<String, History>,
    pub optimistic: HashMap<u64, (String, u32)>,
    pub pending: HashMap<u64, (String, String)>,
    pub failures: HashMap<u64, (String, String, String)>,
    pub recalled: HashSet<String>,
}
impl Model {
    pub fn chat_preview(&self, conversation: &Conversation) -> String {
        let Some(message) = self
            .messages
            .get(&conversation_key(conversation))
            .and_then(|rows| rows.last())
        else {
            return String::new();
        };
        let preview = if self.is_recalled(message) {
            "[已撤回]".into()
        } else {
            summary(&message.elements)
        };
        if matches!(conversation, Conversation::Group { .. }) && !message.is_self {
            format!("{}: {preview}", self.sender_name(message))
        } else {
            preview
        }
    }

    pub fn sorted_chats(&self) -> Vec<&Chat> {
        let mut chats = self.chats.iter().collect::<Vec<_>>();
        chats.sort_by_cached_key(|chat| {
            let key = conversation_key(&chat.conversation);
            let time = self
                .messages
                .get(&key)
                .and_then(|rows| rows.last())
                .map_or(0, |message| message.time);
            (std::cmp::Reverse(time), key)
        });
        chats
    }

    pub fn reply_preview(&self, conversation: &Conversation, reply: &Reply) -> Message {
        if let Some(message) = self
            .messages
            .get(&conversation_key(conversation))
            .and_then(|rows| reply_target(rows, reply))
        {
            return message.clone();
        }
        let own = self.profile.as_ref().is_some_and(|profile| {
            (reply.sender_uin != 0 && reply.sender_uin == profile.uin)
                || (!reply.sender_uid.is_empty() && reply.sender_uid == profile.uid)
        });
        Message {
            conversation: conversation.clone(),
            sender_uin: reply.sender_uin,
            sender_uid: reply.sender_uid.clone(),
            recipient_uin: 0,
            recipient_uid: String::new(),
            sender_name: "未知用户".into(),
            sequence: reply.sequences.first().copied().unwrap_or_default(),
            client_sequence: 0,
            message_id: reply.message_id,
            random: 0,
            time: reply.time.into(),
            is_self: own,
            is_bot: None,
            elements: reply
                .preview(
                    conversation,
                    self.profile.as_ref().map_or(0, |profile| profile.uin),
                )
                .unwrap_or_else(|error| {
                    tracing::warn!(?error, "decode reply preview failed");
                    Vec::new()
                }),
            metadata: vec![],
            raw: vec![],
        }
    }
    pub fn contacts(&mut self, friends: Vec<Friend>, groups: Vec<Group>) {
        for f in &friends {
            self.ensure(
                Conversation::Friend {
                    uin: f.uin,
                    uid: f.uid.clone(),
                },
                if f.remark.is_empty() {
                    f.nickname.clone()
                } else {
                    f.remark.clone()
                },
            );
        }
        for g in &groups {
            self.ensure(
                Conversation::Group { uin: g.uin },
                g.remark
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| g.name.clone()),
            );
        }
        self.friends = friends;
        self.groups = groups;
    }
    pub fn ensure(&mut self, conversation: Conversation, title: String) {
        let key = conversation_key(&conversation);
        if let Some(chat) = self
            .chats
            .iter_mut()
            .find(|chat| conversation_key(&chat.conversation) == key)
        {
            if !title.is_empty() {
                chat.title = title;
            }
        } else {
            self.chats.push(Chat {
                conversation,
                title,
                draft: String::new(),
                unread: 0,
            });
        }
        if self.selected.is_none() {
            self.selected = Some(key);
        }
    }
    pub fn selected_chat(&self) -> Option<&Chat> {
        self.chats
            .iter()
            .find(|chat| Some(&conversation_key(&chat.conversation)) == self.selected.as_ref())
    }
    pub fn stage(
        &mut self,
        id: u64,
        conversation: Conversation,
        elements: Vec<MessageElement>,
        nonce: u32,
        time: i64,
    ) -> Message {
        let (uin, uid, name) = self
            .profile
            .as_ref()
            .map(|profile| (profile.uin, profile.uid.clone(), profile.nickname.clone()))
            .unwrap_or_default();
        let message = Message {
            conversation: conversation.clone(),
            sender_uin: uin,
            sender_uid: uid,
            recipient_uin: 0,
            recipient_uid: String::new(),
            sender_name: name,
            sequence: 0,
            client_sequence: 0,
            message_id: (0x10000000u64 << 32) | nonce as u64,
            random: nonce,
            time,
            is_self: true,
            is_bot: None,
            elements,
            metadata: vec![],
            raw: vec![],
        };
        self.optimistic
            .insert(id, (conversation_key(&conversation), nonce));
        self.receive(message.clone(), false);
        message
    }
    pub fn confirm(&mut self, id: u64, mut message: Message) {
        if let Some((key, nonce)) = self.optimistic.remove(&id)
            && let Some(rows) = self.messages.get_mut(&key)
        {
            if message.time == 0 {
                message.time = rows
                    .iter()
                    .find(|row| row.random == nonce)
                    .map_or(0, |row| row.time);
            }
            rows.retain(|row| !(row.raw.is_empty() && row.is_self && row.random == nonce));
        }
        self.receive(message, false);
    }
    pub fn is_recalled(&self, message: &Message) -> bool {
        self.recalled.contains(&format!(
            "{}:{}",
            conversation_key(&message.conversation),
            message.sequence
        )) || message.is_recalled()
    }
    pub fn receive(&mut self, mut message: Message, live: bool) {
        message.normalize_identity();
        let key = conversation_key(&message.conversation);
        if message.is_recalled() {
            self.recalled.insert(format!("{key}:{}", message.sequence));
        }
        if !self
            .chats
            .iter()
            .any(|chat| conversation_key(&chat.conversation) == key)
        {
            self.ensure(message.conversation.clone(), key.clone());
        }
        let rows = self.messages.entry(key.clone()).or_default();
        if let Some(old) = rows.iter_mut().find(|old| same_message(old, &message)) {
            if message.raw.len() >= old.raw.len() {
                *old = message;
                rows.sort_by_key(|message| (message.time, message.sequence));
            }
            return;
        }
        if live
            && !message.is_self
            && self.selected.as_ref() != Some(&key)
            && let Some(chat) = self
                .chats
                .iter_mut()
                .find(|c| conversation_key(&c.conversation) == key)
        {
            chat.unread += 1;
        }
        rows.push(message);
        rows.sort_by_key(|message| (message.time, message.sequence));
    }
    pub fn sender_uin(&self, message: &Message) -> u64 {
        if message.sender_uin != 0 {
            return message.sender_uin;
        }
        if message.is_self {
            return self.profile.as_ref().map_or(0, |profile| profile.uin);
        }
        if let Some(friend) = self
            .friends
            .iter()
            .find(|friend| friend.uin != 0 && friend.uid == message.sender_uid)
        {
            return friend.uin;
        }
        if let Some(uin) = self
            .resolved_uins
            .get(&message.sender_uid)
            .filter(|uin| **uin != 0)
        {
            return *uin;
        }
        if let Conversation::Group { uin } = message.conversation
            && let Some(member) = self.members.get(&uin).and_then(|members| {
                members
                    .iter()
                    .find(|member| member.uid == message.sender_uid)
            })
        {
            return member.uin;
        }
        self.members
            .values()
            .flatten()
            .find(|member| {
                member.uin != 0
                    && !message.sender_uid.is_empty()
                    && member.uid == message.sender_uid
            })
            .map_or(0, |member| member.uin)
    }
    pub fn sender_name(&self, message: &Message) -> String {
        if message.is_self && !matches!(message.conversation, Conversation::Group { .. }) {
            return self
                .profile
                .as_ref()
                .map(|profile| profile.nickname.clone())
                .unwrap_or_else(|| "我".into());
        }
        let friend = self.friends.iter().find(|f| {
            (!message.sender_uid.is_empty() && f.uid == message.sender_uid)
                || (message.sender_uin != 0 && f.uin == message.sender_uin)
        });
        let member = match message.conversation {
            Conversation::Group { uin } => self.members.get(&uin).and_then(|members| {
                members.iter().find(|m| {
                    (!message.sender_uid.is_empty() && m.uid == message.sender_uid)
                        || (message.sender_uin != 0 && m.uin == message.sender_uin)
                })
            }),
            _ => None,
        };
        display_name(
            friend,
            member,
            if message.is_self {
                self.profile
                    .as_ref()
                    .map(|profile| profile.nickname.as_str())
                    .unwrap_or(&message.sender_name)
            } else {
                &message.sender_name
            },
            self.sender_uin(message),
        )
    }
}

pub fn reply_target<'a>(messages: &'a [Message], reply: &Reply) -> Option<&'a Message> {
    messages.iter().find(|message| {
        let sender = if !reply.sender_uid.is_empty() && !message.sender_uid.is_empty() {
            reply.sender_uid == message.sender_uid
        } else if reply.sender_uin != 0 && message.sender_uin != 0 {
            reply.sender_uin == message.sender_uin
        } else {
            true
        };
        sender
            && ((message.reply_sequence() != 0
                && reply.sequences.contains(&message.reply_sequence()))
                || (message.sequence != 0 && reply.sequences.contains(&message.sequence))
                || (reply.message_id != 0 && reply.message_id == message.message_id))
    })
}

pub fn image_only(message: &Message) -> bool {
    !message.elements.is_empty()
        && message
            .elements
            .iter()
            .all(|element| matches!(element, MessageElement::Image(_)))
}

pub fn message_groups<'a>(
    messages: &'a [Message],
    recalled: &HashSet<String>,
) -> Vec<&'a [Message]> {
    let eligible = |message: &Message| {
        image_only(message)
            && message.elements.len() == 1
            && !recalled.contains(&format!(
                "{}:{}",
                conversation_key(&message.conversation),
                message.sequence
            ))
    };
    let mut groups = Vec::new();
    let mut start = 0;
    for index in 1..=messages.len() {
        let join = index < messages.len()
            && eligible(&messages[index - 1])
            && eligible(&messages[index])
            && messages[index - 1].conversation == messages[index].conversation
            && same_sender(&messages[index - 1], &messages[index]);
        if !join {
            groups.push(&messages[start..index]);
            start = index;
        }
    }
    groups
}

fn same_sender(left: &Message, right: &Message) -> bool {
    (left.is_self && right.is_self)
        || (!left.sender_uid.is_empty() && left.sender_uid == right.sender_uid)
        || (left.sender_uin != 0 && left.sender_uin == right.sender_uin)
}
pub fn display_name(
    friend: Option<&Friend>,
    member: Option<&GroupMember>,
    fallback: &str,
    uin: u64,
) -> String {
    friend
        .map(|f| f.nickname.as_str())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            member
                .and_then(|m| m.card.as_deref())
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            member
                .map(|m| m.nickname.as_str())
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| (!fallback.trim().is_empty()).then_some(fallback))
        .map(str::to_owned)
        .unwrap_or_else(|| uin.to_string())
}
pub fn summary(elements: &[MessageElement]) -> String {
    elements
        .iter()
        .map(|element| match element {
            MessageElement::Text(text) => text.clone(),
            MessageElement::Mention(m) => format!("@{}", m.display),
            MessageElement::Reply(_) => "[回复] ".into(),
            MessageElement::Image(_) => "[图片]".into(),
            MessageElement::Voice(_) => "[语音]".into(),
            MessageElement::Video(_) => "[视频]".into(),
            MessageElement::File(f) => format!("[文件] {}", f.name),
            MessageElement::LightApp(_) => "[卡片]".into(),
            MessageElement::Forward(_) => "[聊天记录]".into(),
            MessageElement::GroupFile(f) => format!("[群文件] {}", f.name),
            MessageElement::Opaque(_) => "[其他消息]".into(),
        })
        .collect::<String>()
}

fn same_message(left: &Message, right: &Message) -> bool {
    let sender = (left.is_self && right.is_self)
        || (!left.sender_uid.is_empty() && left.sender_uid == right.sender_uid)
        || (left.sender_uin != 0 && left.sender_uin == right.sender_uin);
    if !sender {
        return false;
    }
    if left.message_id != 0 && left.message_id == right.message_id {
        return true;
    }
    let close = left.time == 0 || right.time == 0 || left.time.abs_diff(right.time) <= 10;
    if !close {
        return false;
    }
    // An acknowledgement has no wire body; its generated ID may differ from the echo.
    if !left.raw.is_empty()
        && !right.raw.is_empty()
        && left.message_id != 0
        && right.message_id != 0
    {
        return false;
    }
    let nonce = |message: &Message| {
        if message.random != 0 {
            message.random
        } else if message.is_self {
            message.message_id as u32
        } else {
            0
        }
    };
    let (left_nonce, right_nonce) = (nonce(left), nonce(right));
    if left_nonce != 0 && right_nonce != 0 {
        return left_nonce == right_nonce;
    }
    if left.sequence != 0 && left.sequence == right.sequence {
        return true;
    }
    (left.random != 0 && left.random == right.random)
        || (left.client_sequence != 0 && left.client_sequence == right.client_sequence)
}

pub fn can_recall(message: &Message, now: i64) -> bool {
    let age = now.saturating_sub(message.time);
    message.is_self && message.sequence != 0 && message.time > 0 && (0..120).contains(&age)
}
