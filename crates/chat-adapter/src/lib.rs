//! Protocol-neutral adapter contracts.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
};
use tokio::sync::broadcast;

pub type Result<T> = std::result::Result<T, Error>;
pub type Task<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum Error {
    #[error("Unsupported operation: {0}")]
    Unsupported(String),
    #[error("Invalid request: {0}")]
    Invalid(String),
    #[error("Authentication required")]
    Unauthorized,
    #[error("{message}")]
    Remote {
        code: String,
        message: String,
        retry_after_seconds: Option<u32>,
    },
    #[error("Delivery is unconfirmed: {0}")]
    Unconfirmed(String),
    #[error("{0}")]
    Transport(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AccountId {
    pub adapter: String,
    /// Adapter-local account key, stable across reconnects.
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChatId {
    pub account: AccountId,
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageId {
    pub chat: ChatId,
    pub id: String,
}
impl AccountId {
    pub fn chat(&self, id: impl Into<String>) -> ChatId {
        ChatId {
            account: self.clone(),
            id: id.into(),
        }
    }
    pub fn require_chat(&self, chat: &ChatId) -> Result<()> {
        if &chat.account == self {
            Ok(())
        } else {
            Err(Error::Invalid("Account mismatch".into()))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub avatar: Option<Box<Media>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatKind {
    Direct,
    Group,
    Channel,
    Saved,
    Temporary,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ChatId,
    pub kind: ChatKind,
    pub title: String,
    pub avatar: Option<Box<Media>>,
    pub unread: u32,
    pub last_activity_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Media {
    /// Opaque, adapter-owned reference.
    pub id: String,
    pub mime: Option<String>,
    pub name: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub byte_length: Option<u64>,
    #[serde(default)]
    pub data: serde_json::Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Content {
    Text(String),
    Image(Media),
    Video(Media),
    Audio(Media),
    Voice(Media),
    File(Media),
    Sticker(Media),
    Extension(ExtensionContent),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    Bubble,
    Card,
    Notice,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DisplayPart {
    Text(String),
    User(User),
    Image(Media),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtensionContent {
    /// Namespaced stable identifier, for example `qq.poke`.
    pub kind: String,
    pub version: u32,
    /// Preserved even if the receiving host has no dedicated renderer.
    pub data: serde_json::Value,
    pub layout: Layout,
    /// Declarative fallback rendering.
    pub display: Vec<DisplayPart>,
}
impl Content {
    pub fn text_preview(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Extension(extension) => extension
                .display
                .iter()
                .map(|part| match part {
                    DisplayPart::Text(text) => text.clone(),
                    DisplayPart::User(user) => user.name.clone(),
                    DisplayPart::Image(image) => image.name.clone().unwrap_or_default(),
                })
                .collect(),
            Self::Image(_) => "[Image]".into(),
            Self::Video(_) => "[Video]".into(),
            Self::Voice(_) => "[Voice]".into(),
            Self::Audio(_) => "[Audio]".into(),
            Self::File(file) => file.name.clone().unwrap_or_else(|| "[File]".into()),
            Self::Sticker(_) => "[Sticker]".into(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub target: MessageId,
    pub sender: User,
    pub preview: Vec<Content>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    /// Correlates optimistic sends with receipts and server echoes when the protocol supports it.
    pub client_id: Option<String>,
    pub sender: User,
    pub time_ms: i64,
    pub outgoing: bool,
    pub content: Vec<Content>,
    pub reply: Option<Box<Reply>>,
    pub deleted: bool,
}
/// Adjacent messages stay independent; this only controls repeated sender decoration.
pub fn continues_sender(previous: &Message, current: &Message) -> bool {
    previous.id.chat == current.id.chat
        && previous.outgoing == current.outgoing
        && !previous.sender.id.is_empty()
        && previous.sender.id == current.sender.id
        && !previous.content.iter().chain(&current.content).any(
            |part| matches!(part,Content::Extension(extension) if extension.layout==Layout::Notice),
        )
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageRequest {
    pub cursor: Option<String>,
    pub limit: u32,
}
impl Default for PageRequest {
    fn default() -> Self {
        Self {
            cursor: None,
            limit: 50,
        }
    }
}
impl PageRequest {
    pub fn validate(&self) -> Result<()> {
        if (1..=100).contains(&self.limit) {
            Ok(())
        } else {
            Err(Error::Invalid("Page limit must be 1..=100".into()))
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}
#[derive(Clone, Debug)]
pub struct SendRequest {
    pub chat: ChatId,
    pub client_id: String,
    pub content: Vec<Content>,
    pub reply_to: Option<MessageId>,
}
#[derive(Clone, Debug)]
pub enum SendOutcome {
    Confirmed(Box<Message>),
    /// The server accepted an operation without returning a message identity.
    Accepted {
        client_id: String,
    },
}

impl SendOutcome {
    pub fn confirmed(message: Message) -> Self {
        Self::Confirmed(Box::new(message))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    Text,
    Secret,
    Integer,
    Phone,
    Code,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginChoice {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AuthState {
    Disconnected,
    Choice {
        step: String,
        choices: Vec<LoginChoice>,
    },
    Browser {
        url: String,
    },
    Form {
        step: String,
        fields: Vec<Field>,
    },
    Qr {
        url: String,
        expires_ms: Option<i64>,
        /// Optional alternative login steps. Selecting one cancels the pending challenge.
        #[serde(default)]
        choices: Vec<LoginChoice>,
    },
    Ready {
        account: AccountId,
        user: User,
    },
    Closed,
}
pub struct AuthInput {
    pub step: String,
    pub fields: BTreeMap<String, String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    Account,
    Conversation,
    Message,
    User,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub scope: Scope,
    pub fields: Vec<Field>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Descriptor {
    pub id: String,
    pub name: String,
    /// Namespaced capabilities are extensible without changing this crate.
    pub capabilities: BTreeSet<String>,
    pub actions: Vec<Action>,
}
#[derive(Clone, Debug)]
pub struct ExtensionRequest {
    pub action: String,
    pub chat: Option<ChatId>,
    pub message: Option<MessageId>,
    pub user: Option<String>,
    pub data: serde_json::Value,
}
#[derive(Clone, Debug)]
pub enum Event {
    Auth(AuthState),
    Conversation(Conversation),
    Message(Message),
    Deleted(Vec<MessageId>),
    Error(Error),
    /// A host must fetch updates/history after a disconnect or a lagged broadcast receiver.
    ResyncRequired,
}

/// A single account/session.
pub trait Adapter: Send + Sync {
    fn descriptor(&self) -> Descriptor;
    fn account(&self) -> AccountId;
    fn subscribe(&self) -> broadcast::Receiver<Event>;
    fn authenticate(&self, input: AuthInput) -> Task<'_, AuthState>;
    fn conversations(&self, page: PageRequest) -> Task<'_, Page<Conversation>>;
    fn history(&self, chat: ChatId, page: PageRequest) -> Task<'_, Page<Message>>;
    fn send(&self, request: SendRequest) -> Task<'_, SendOutcome>;
    fn download(&self, media: Media, destination: PathBuf) -> Task<'_, PathBuf>;
    fn execute(&self, request: ExtensionRequest) -> Task<'_, serde_json::Value>;
    /// Ends tasks and releases the session. Idempotent.
    fn close(&self) -> Task<'_, ()>;
}

/// Structured records; a host may encode these as protobuf in its own database.
pub trait Store: Send + Sync {
    fn get(&self, key: String) -> Task<'_, Option<serde_json::Value>>;
    fn put(&self, key: String, value: serde_json::Value) -> Task<'_, ()>;
    fn delete(&self, key: String) -> Task<'_, ()>;
}

#[derive(Default)]
pub struct Registry {
    adapters: BTreeMap<AccountId, Arc<dyn Adapter>>,
}
impl Registry {
    pub fn insert(&mut self, adapter: Arc<dyn Adapter>) -> Result<()> {
        let account = adapter.account();
        if account.adapter != adapter.descriptor().id {
            return Err(Error::Invalid("Adapter identity mismatch".into()));
        }
        if self.adapters.contains_key(&account) {
            return Err(Error::Invalid("Account already registered".into()));
        }
        self.adapters.insert(account, adapter);
        Ok(())
    }
    pub fn get(&self, account: &AccountId) -> Result<Arc<dyn Adapter>> {
        self.adapters
            .get(account)
            .cloned()
            .ok_or_else(|| Error::Invalid("Unknown account".into()))
    }
    pub fn remove(&mut self, account: &AccountId) -> Option<Arc<dyn Adapter>> {
        self.adapters.remove(account)
    }
    pub fn accounts(&self) -> impl Iterator<Item = &AccountId> {
        self.adapters.keys()
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Timeline {
    pub messages: BTreeMap<MessageId, Message>,
    #[serde(default)]
    pub deleted: BTreeSet<MessageId>,
}
impl Timeline {
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Message(message) => {
                if let Some(client_id) = &message.client_id {
                    self.messages.retain(|id, old| {
                        id == &message.id
                            || id.chat != message.id.chat
                            || old.client_id.as_ref() != Some(client_id)
                    });
                }
                let mut message = message.clone();
                if self.deleted.contains(&message.id)
                    || self
                        .messages
                        .get(&message.id)
                        .is_some_and(|old| old.deleted)
                {
                    message.deleted = true;
                }
                self.messages.insert(message.id.clone(), message);
            }
            Event::Deleted(ids) => {
                for id in ids {
                    self.deleted.insert(id.clone());
                    if let Some(message) = self.messages.get_mut(id) {
                        message.deleted = true;
                    }
                }
            }
            _ => {}
        }
    }
    pub fn in_chat(&self, chat: &ChatId) -> Vec<&Message> {
        let mut messages: Vec<_> = self
            .messages
            .values()
            .filter(|message| &message.id.chat == chat)
            .collect();
        messages.sort_by(|a, b| (a.time_ms, &a.id).cmp(&(b.time_ms, &b.id)));
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message(adapter: &str, account: &str, id: &str) -> Message {
        let account = AccountId {
            adapter: adapter.into(),
            id: account.into(),
        };
        Message {
            id: MessageId {
                chat: account.chat("42"),
                id: id.into(),
            },
            client_id: Some("nonce".into()),
            sender: User {
                id: "7".into(),
                name: "Alice".into(),
                avatar: None,
            },
            time_ms: 1,
            outgoing: false,
            content: vec![Content::Text("Hello".into())],
            reply: None,
            deleted: false,
        }
    }
    #[test]
    fn adjacent_sender_decoration_does_not_merge_or_cross_accounts_or_notices() {
        let a = message("matrix", "one", "1");
        let mut b = message("matrix", "one", "2");
        assert!(continues_sender(&a, &b));
        b.sender.name = "new display name".into();
        assert!(continues_sender(&a, &b));
        b.sender.id = "different".into();
        assert!(!continues_sender(&a, &b));
        assert!(!continues_sender(&a, &message("matrix", "two", "2")));
        assert!(!continues_sender(&a, &message("telegram", "one", "2")));
        let mut notice = a.clone();
        notice.content = vec![Content::Extension(ExtensionContent {
            kind: "custom.notice".into(),
            version: 1,
            data: Default::default(),
            layout: Layout::Notice,
            display: vec![],
        })];
        assert!(!continues_sender(&a, &notice));
        assert!(!continues_sender(&notice, &a));
    }
    #[test]
    fn accounts_and_protocols_are_isolated_and_echoes_replace_optimistic_rows() {
        let mut timeline = Timeline::default();
        for (adapter, account) in [("qq", "1"), ("qq", "2"), ("telegram", "1")] {
            timeline.apply(&Event::Message(message(adapter, account, "pending")));
            timeline.apply(&Event::Message(message(adapter, account, "server")));
        }
        assert_eq!(timeline.messages.len(), 3);
        let row = message("qq", "1", "server");
        timeline.apply(&Event::Deleted(vec![row.id.clone()]));
        timeline.apply(&Event::Message(row.clone()));
        assert!(timeline.messages[&row.id].deleted);
        assert!(!timeline.messages[&message("telegram", "1", "server").id].deleted);
        let later = message("qq", "1", "older-history");
        timeline.apply(&Event::Deleted(vec![later.id.clone()]));
        timeline.apply(&Event::Message(later.clone()));
        assert!(timeline.messages[&later.id].deleted);
    }
    #[test]
    fn unknown_extensions_keep_payload_and_can_be_rendered_without_platform_code() {
        let content = Content::Extension(ExtensionContent {
            kind: "example.tap".into(),
            version: 2,
            data: serde_json::json!({"arbitrary": [1,2,3]}),
            layout: Layout::Notice,
            display: vec![
                DisplayPart::User(User {
                    id: "42".into(),
                    name: "Alice".into(),
                    avatar: None,
                }),
                DisplayPart::Text(" tapped Bob".into()),
            ],
        });
        let restored: Content =
            serde_json::from_slice(&serde_json::to_vec(&content).unwrap()).unwrap();
        assert_eq!(restored, content);
        assert_eq!(restored.text_preview(), "Alice tapped Bob");
    }
    #[test]
    fn cross_account_requests_and_unbounded_pages_are_rejected() {
        let a = AccountId {
            adapter: "qq".into(),
            id: "1".into(),
        };
        let b = AccountId {
            adapter: "telegram".into(),
            id: "1".into(),
        };
        assert!(a.require_chat(&b.chat("42")).is_err());
        assert!(
            PageRequest {
                limit: 0,
                cursor: None
            }
            .validate()
            .is_err()
        );
        assert!(
            PageRequest {
                limit: 101,
                cursor: None
            }
            .validate()
            .is_err()
        );
    }
}

pub mod mock;

pub mod login;
pub use login::{LoginMachine, LoginSnapshot};
