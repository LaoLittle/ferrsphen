//! Deterministic, offline adapter used by hosts to exercise the same API as network adapters.
use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<String, serde_json::Value>>);
impl Store for MemoryStore {
    fn get(&self, key: String) -> Task<'_, Option<serde_json::Value>> {
        Box::pin(async move { Ok(self.0.lock().unwrap().get(&key).cloned()) })
    }
    fn put(&self, key: String, value: serde_json::Value) -> Task<'_, ()> {
        Box::pin(async move {
            self.0.lock().unwrap().insert(key, value);
            Ok(())
        })
    }
    fn delete(&self, key: String) -> Task<'_, ()> {
        Box::pin(async move {
            self.0.lock().unwrap().remove(&key);
            Ok(())
        })
    }
}
pub struct MockAdapter {
    account: AccountId,
    events: broadcast::Sender<Event>,
    messages: Mutex<Vec<Message>>,
    sequence: AtomicU64,
}
impl MockAdapter {
    pub fn new(account: impl Into<String>) -> Self {
        let account = AccountId {
            adapter: "mock".into(),
            id: account.into(),
        };
        let mut initial = Message {
            id: MessageId {
                chat: account.chat("room"),
                id: "1".into(),
            },
            client_id: None,
            sender: User {
                id: "alice".into(),
                name: "Alice".into(),
                avatar: None,
            },
            time_ms: 1,
            outgoing: false,
            reply: None,
            deleted: false,
            content: vec![Content::Text("Hello".into())],
        };
        let mut notice = initial.clone();
        notice.id.id = "2".into();
        notice.time_ms = 2;
        notice.content = vec![Content::Extension(ExtensionContent {
            kind: "example.poke".into(),
            version: 1,
            data: serde_json::json!({"actor":"alice","target":"me"}),
            layout: Layout::Notice,
            display: vec![
                DisplayPart::User(initial.sender.clone()),
                DisplayPart::Text(" tapped you".into()),
            ],
        })];
        initial.client_id = None;
        Self {
            account,
            events: broadcast::channel(64).0,
            messages: Mutex::new(vec![initial, notice]),
            sequence: AtomicU64::new(2),
        }
    }
    fn conversation(&self) -> Conversation {
        Conversation {
            id: self.account.chat("room"),
            title: "Alice".into(),
            kind: ChatKind::Direct,
            avatar: None,
            unread: 0,
            last_activity_ms: self.sequence.load(Ordering::Relaxed) as i64,
        }
    }
}
impl Adapter for MockAdapter {
    fn account(&self) -> AccountId {
        self.account.clone()
    }
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            id: "mock".into(),
            name: "Mock".into(),
            capabilities: BTreeSet::from(["core.text".into(), "example.poke".into()]),
            actions: vec![Action {
                id: "example.poke".into(),
                label: "Poke".into(),
                scope: Scope::Conversation,
                fields: vec![],
            }],
        }
    }
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
    fn authenticate(&self, _: AuthInput) -> Task<'_, AuthState> {
        Box::pin(async move {
            Ok(AuthState::Ready {
                account: self.account.clone(),
                user: User {
                    id: "me".into(),
                    name: "You".into(),
                    avatar: None,
                },
            })
        })
    }
    fn conversations(&self, page: PageRequest) -> Task<'_, Page<Conversation>> {
        Box::pin(async move {
            page.validate()?;
            Ok(Page {
                items: if page.cursor.is_none() {
                    vec![self.conversation()]
                } else {
                    vec![]
                },
                next: None,
            })
        })
    }
    fn history(&self, chat: ChatId, page: PageRequest) -> Task<'_, Page<Message>> {
        Box::pin(async move {
            self.account.require_chat(&chat)?;
            page.validate()?;
            let before = page
                .cursor
                .as_deref()
                .map(|cursor| {
                    cursor
                        .parse::<u64>()
                        .map_err(|_| Error::Invalid("Invalid cursor".into()))
                })
                .transpose()?
                .unwrap_or(u64::MAX);
            let messages = self.messages.lock().unwrap();
            let mut items: Vec<_> = messages
                .iter()
                .rev()
                .filter(|message| {
                    message.id.chat == chat && message.id.id.parse::<u64>().unwrap() < before
                })
                .take(page.limit as usize + 1)
                .cloned()
                .collect();
            let next = if items.len() > page.limit as usize {
                items.pop();
                items.last().map(|message| message.id.id.clone())
            } else {
                None
            };
            Ok(Page { items, next })
        })
    }
    fn send(&self, request: SendRequest) -> Task<'_, SendOutcome> {
        Box::pin(async move {
            self.account.require_chat(&request.chat)?;
            if request.content.is_empty() {
                return Err(Error::Invalid("Empty message".into()));
            }
            let mut messages = self.messages.lock().unwrap();
            if let Some(existing) = messages.iter().find(|message| {
                message.id.chat == request.chat
                    && message.client_id.as_ref() == Some(&request.client_id)
            }) {
                return Ok(SendOutcome::confirmed(existing.clone()));
            }
            let id = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let message = Message {
                id: MessageId {
                    chat: request.chat,
                    id: id.to_string(),
                },
                client_id: Some(request.client_id),
                sender: User {
                    id: "me".into(),
                    name: "You".into(),
                    avatar: None,
                },
                time_ms: id as i64,
                outgoing: true,
                content: request.content,
                reply: None,
                deleted: false,
            };
            messages.push(message.clone());
            let _ = self.events.send(Event::Message(message.clone()));
            Ok(SendOutcome::confirmed(message))
        })
    }
    fn download(&self, _: Media, _: PathBuf) -> Task<'_, PathBuf> {
        Box::pin(async { Err(Error::Unsupported("Mock media download".into())) })
    }
    fn execute(&self, request: ExtensionRequest) -> Task<'_, serde_json::Value> {
        Box::pin(async move {
            if request.action != "example.poke" {
                return Err(Error::Unsupported(request.action));
            }
            let chat = request
                .chat
                .ok_or_else(|| Error::Invalid("Select a conversation".into()))?;
            self.account.require_chat(&chat)?;
            let id = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let message = Message {
                id: MessageId {
                    chat,
                    id: id.to_string(),
                },
                client_id: None,
                sender: User {
                    id: "me".into(),
                    name: "You".into(),
                    avatar: None,
                },
                time_ms: id as i64,
                outgoing: true,
                content: vec![Content::Extension(ExtensionContent {
                    kind: "example.poke".into(),
                    version: 1,
                    data: request.data,
                    layout: Layout::Notice,
                    display: vec![DisplayPart::Text("You tapped Alice".into())],
                })],
                reply: None,
                deleted: false,
            };
            self.messages.lock().unwrap().push(message.clone());
            let _ = self.events.send(Event::Message(message));
            Ok(serde_json::Value::Null)
        })
    }
    fn close(&self) -> Task<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn trait_objects_registry_pages_and_extensions_work_without_a_client_dependency() {
        let adapter: Arc<dyn Adapter> = Arc::new(MockAdapter::new("one"));
        let mut registry = Registry::default();
        registry.insert(adapter.clone()).unwrap();
        assert!(registry.insert(adapter.clone()).is_err());
        let mut events = adapter.subscribe();
        let chat = adapter
            .conversations(PageRequest::default())
            .await
            .unwrap()
            .items
            .remove(0)
            .id;
        let first = adapter
            .history(
                chat.clone(),
                PageRequest {
                    cursor: None,
                    limit: 1,
                },
            )
            .await
            .unwrap();
        assert!(first.items[0].content[0].text_preview().contains("Alice"));
        let second = adapter
            .history(
                chat.clone(),
                PageRequest {
                    cursor: first.next,
                    limit: 1,
                },
            )
            .await
            .unwrap();
        assert_eq!(second.items[0].id.id, "1");
        assert!(second.next.is_none());
        adapter
            .execute(ExtensionRequest {
                action: "example.poke".into(),
                chat: Some(chat),
                message: None,
                user: None,
                data: serde_json::json!({"custom":true}),
            })
            .await
            .unwrap();
        let Event::Message(message) = events.recv().await.unwrap() else {
            panic!()
        };
        assert_eq!(message.content[0].text_preview(), "You tapped Alice");
    }
}
