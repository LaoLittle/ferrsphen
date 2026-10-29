//! Deterministic, offline adapter implementing the network adapter API.
use super::*;
use std::sync::Mutex;
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
/// 2026-10-28 09:00 (UTC+8). Fixture timestamps are fixed offsets from here.
const BASE_MS: i64 = 1_793_149_200_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Offline account with fixed contacts, groups and histories.
pub struct MockAdapter {
    account: AccountId,
    events: broadcast::Sender<Event>,
    /// `last_activity_ms` is derived from the messages when listed.
    conversations: Vec<Conversation>,
    state: Mutex<State>,
}
struct State {
    /// Ascending ids across all chats; the numeric id doubles as the history cursor.
    messages: Vec<Message>,
    sequence: u64,
}
fn user(id: &str, name: &str) -> User {
    User {
        id: id.into(),
        name: name.into(),
        avatar: None,
    }
}
fn me() -> User {
    user("me", "You")
}
struct Entry {
    chat: &'static str,
    time_ms: i64,
    sender: User,
    content: Content,
    reply: Option<usize>,
}
/// Collects messages in any order; `finish` sorts them by time and assigns ids.
#[derive(Default)]
struct Fixture(Vec<Entry>);
impl Fixture {
    fn push(&mut self, chat: &'static str, time_ms: i64, sender: &User, content: Content) -> usize {
        self.0.push(Entry {
            chat,
            time_ms,
            sender: sender.clone(),
            content,
            reply: None,
        });
        self.0.len() - 1
    }
    fn text(&mut self, chat: &'static str, time_ms: i64, sender: &User, text: &str) -> usize {
        self.push(chat, time_ms, sender, Content::Text(text.into()))
    }
    fn reply(
        &mut self,
        chat: &'static str,
        time_ms: i64,
        sender: &User,
        text: &str,
        target: usize,
    ) -> usize {
        let entry = self.text(chat, time_ms, sender, text);
        self.0[entry].reply = Some(target);
        entry
    }
    fn notice(
        &mut self,
        chat: &'static str,
        time_ms: i64,
        sender: &User,
        kind: &str,
        data: serde_json::Value,
        display: Vec<DisplayPart>,
    ) -> usize {
        self.push(
            chat,
            time_ms,
            sender,
            Content::Extension(ExtensionContent {
                kind: kind.into(),
                version: 1,
                data,
                layout: Layout::Notice,
                display,
            }),
        )
    }
    fn finish(self, account: &AccountId) -> Vec<Message> {
        let mut order: Vec<usize> = (0..self.0.len()).collect();
        order.sort_by_key(|&index| (self.0[index].time_ms, index));
        let mut position = vec![0; self.0.len()];
        for (rank, &index) in order.iter().enumerate() {
            position[index] = rank + 1;
        }
        let id = |index: usize| MessageId {
            chat: account.chat(self.0[index].chat),
            id: position[index].to_string(),
        };
        order
            .into_iter()
            .map(|index| {
                let entry = &self.0[index];
                Message {
                    id: id(index),
                    client_id: None,
                    sender: entry.sender.clone(),
                    time_ms: entry.time_ms,
                    outgoing: entry.sender.id == "me",
                    content: vec![entry.content.clone()],
                    reply: entry.reply.map(|target| {
                        Box::new(Reply {
                            target: id(target),
                            sender: self.0[target].sender.clone(),
                            preview: vec![self.0[target].content.clone()],
                        })
                    }),
                    deleted: false,
                }
            })
            .collect()
    }
}
const TEAM_LINES: [&str; 30] = [
    "早，今天站会改到 10 点",
    "收到 👌",
    "登录页的新设计稿已经更新到 Figma 了",
    "我看了一下，深色模式下按钮对比度有点低",
    "这个我来改",
    "CI 又挂了，好像是 macOS runner 的证书过期",
    "已经重新跑了，绿了 ✅",
    "接口文档在 wiki 上，分页参数改成 cursor 了",
    "那客户端这边要跟着改吗？",
    "要的，旧的 offset 下周下线",
    "好，我今天下午提 PR",
    "午饭吃什么？🍜",
    "楼下新开的拉面店不错",
    "+1",
    "我晚点到，先帮我占个位",
    "性能测试报告出来了，冷启动 1.2s",
    "比上个版本快了 30% 🎉",
    "Android 上滚动还是有点掉帧",
    "我怀疑是图片解码在主线程",
    "明天我抓个 trace 看看",
    "发版时间定在周五晚上",
    "周五晚上？😅",
    "那就周五下午三点吧",
    "OK",
    "文案的翻译还缺日语和韩语",
    "我找人帮忙翻一下",
    "记得更新 changelog",
    "已更新",
    "大家辛苦了 🙏",
    "明天见 👋",
];
/// Indexes into the team members; runs of the same index share a sender group.
const TEAM_SENDERS: [usize; 16] = [0, 0, 1, 2, 4, 1, 1, 1, 3, 4, 4, 0, 2, 2, 3, 1];
fn fixture(account: &AccountId) -> (Vec<Conversation>, Vec<Message>) {
    let me = me();
    let alice = user("alice", "Alice");
    let bob = user("bob", "Bob Chen");
    let carol = user("carol", "Carol");
    let zhang = user("zhangwei", "张伟");
    let li = user("lina", "李娜");
    let wang = user("wangqiang", "王强");
    let mom = user("mom", "妈妈");
    let dad = user("dad", "爸爸");
    let sister = user("sister", "小妹");
    let mut f = Fixture::default();

    let day = BASE_MS - 6 * DAY + 10 * HOUR;
    f.text(
        "carol",
        day + 10 * MINUTE,
        &carol,
        "Thanks for the book recommendation 📚",
    );
    f.text("carol", day + 12 * MINUTE, &me, "Which one did you pick?");
    f.text(
        "carol",
        day + 15 * MINUTE,
        &carol,
        "《三体》! Halfway through already",
    );
    f.text(
        "carol",
        day + 16 * MINUTE,
        &me,
        "Wait until you get to the second book 😄",
    );

    let day = BASE_MS - DAY;
    f.text(
        "bob",
        day + 62 * MINUTE,
        &bob,
        "Morning! Did you get a chance to look at the adapter PR?",
    );
    f.text(
        "bob",
        day + 65 * MINUTE,
        &me,
        "Not yet, will do after lunch",
    );
    let review = f.text(
        "bob",
        day + 5 * HOUR + 30 * MINUTE,
        &me,
        "Left a few comments on the PR:\n\n\
         1. `history()` should validate the cursor before locking the store.\n\
         2. The retry loop never backs off — please add a jittered delay.\n\
         3. Tests: cover the empty-page case and an invalid cursor.\n\
         4. Nit: rename `do_thing` to something descriptive.\n\n\
         Otherwise looks great, nice work! 🚀",
    );
    f.reply(
        "bob",
        day + 5 * HOUR + 41 * MINUTE,
        &bob,
        "Thanks for the thorough review, fixing now",
        review,
    );
    f.text(
        "bob",
        day + 5 * HOUR + 42 * MINUTE,
        &bob,
        "Pushed the fixes for 1 and 3",
    );
    f.text(
        "bob",
        day + 5 * HOUR + 42 * MINUTE + 20_000,
        &bob,
        "2 needs a bit more thought, I'll ping you tomorrow",
    );
    f.text("bob", day + 8 * HOUR + 55 * MINUTE, &me, "Sounds good 👍");

    let alice_day = BASE_MS - DAY + 9 * HOUR;
    f.text("room", alice_day + 20 * MINUTE, &alice, "Hello");
    f.text("room", alice_day + 21 * MINUTE, &me, "Hi Alice! 好久不见");
    f.text(
        "room",
        alice_day + 21 * MINUTE + 30_000,
        &alice,
        "周末有空吗？想约你去看展 🎨",
    );
    f.text(
        "room",
        alice_day + 22 * MINUTE,
        &alice,
        "就是那个 teamLab 的沉浸式展览",
    );
    f.text("room", alice_day + 25 * MINUTE, &me, "好呀！周六下午可以");
    f.text("room", alice_day + 26 * MINUTE, &alice, "👍 那我先订票");

    let members = [&zhang, &li, &wang, &bob, &me];
    f.notice(
        "team",
        BASE_MS - 10 * MINUTE,
        &wang,
        "mock.member_joined",
        serde_json::json!({"user":"wangqiang"}),
        vec![
            DisplayPart::User(wang.clone()),
            DisplayPart::Text(" 加入了群聊".into()),
        ],
    );
    for index in 0..64 {
        let time_ms = BASE_MS + index as i64 * 5 * MINUTE + (index % 3) as i64 * 47_000;
        let sender = members[TEAM_SENDERS[index % TEAM_SENDERS.len()]];
        f.text(
            "team",
            time_ms,
            sender,
            TEAM_LINES[index % TEAM_LINES.len()],
        );
    }
    let minutes = f.text(
        "team",
        BASE_MS + 5 * HOUR + 30 * MINUTE,
        &li,
        "今天的会议纪要：\n\
         1. 登录页深色模式对比度问题 —— 张伟负责，周三前完成\n\
         2. 分页接口迁移到 cursor —— 客户端本周内合入\n\
         3. Android 滚动掉帧 —— 王强明天抓 trace\n\
         4. 发版时间：周五 15:00\n\
         有遗漏的话直接在群里补充 🙌",
    );
    f.reply(
        "team",
        BASE_MS + 5 * HOUR + 34 * MINUTE,
        &me,
        "补充一条：日语和韩语翻译我来跟进",
        minutes,
    );
    f.text("team", BASE_MS + 5 * HOUR + 35 * MINUTE, &zhang, "👍👍");

    let evening = BASE_MS + 11 * HOUR;
    f.text("family", evening, &mom, "晚饭吃了吗？");
    f.text("family", evening + MINUTE, &mom, "今天包了饺子 🥟");
    f.text(
        "family",
        evening + MINUTE + 30_000,
        &mom,
        "下次回来给你们留一些",
    );
    f.text(
        "family",
        evening + 5 * MINUTE,
        &sister,
        "哇！我要韭菜鸡蛋馅的 😋",
    );
    let park = f.text(
        "family",
        evening + 6 * MINUTE,
        &dad,
        "周日全家去公园走走？天气预报说是晴天 ☀️",
    );
    f.text("family", evening + 10 * MINUTE, &me, "好呀，我周日有空");
    f.reply("family", evening + 12 * MINUTE, &sister, "我也去！", park);
    f.text("family", evening + 90 * MINUTE, &mom, "早点休息，晚安 🌙");

    let morning = BASE_MS + DAY - 20 * MINUTE;
    let tickets = f.text("room", morning, &alice, "票订好了，周六 14:00 入场");
    f.text(
        "room",
        morning + MINUTE,
        &alice,
        "地址：上海市徐汇区龙腾大道 2555 号\n地铁 11 号线云锦路站 2 号口出\n到了给我发消息～",
    );
    f.reply(
        "room",
        morning + 10 * MINUTE,
        &me,
        "收到，我会提前十分钟到 ⏰",
        tickets,
    );
    f.text("room", morning + 48 * MINUTE, &alice, "在吗？");
    f.notice(
        "room",
        morning + 50 * MINUTE,
        &alice,
        "example.poke",
        serde_json::json!({"actor":"alice","target":"me"}),
        vec![
            DisplayPart::User(alice.clone()),
            DisplayPart::Text(" tapped you".into()),
        ],
    );

    let conversation = |id: &str, title: &str, kind, unread| Conversation {
        id: account.chat(id),
        kind,
        title: title.into(),
        avatar: None,
        unread,
        last_activity_ms: 0,
    };
    (
        vec![
            conversation("room", "Alice", ChatKind::Direct, 2),
            conversation("team", "产品研发群", ChatKind::Group, 12),
            conversation("family", "相亲相爱一家人 🏡", ChatKind::Group, 3),
            conversation("bob", "Bob Chen", ChatKind::Direct, 0),
            conversation("carol", "Carol", ChatKind::Direct, 0),
        ],
        f.finish(account),
    )
}
impl MockAdapter {
    pub fn new(account: impl Into<String>) -> Self {
        let account = AccountId {
            adapter: "mock".into(),
            id: account.into(),
        };
        let (conversations, messages) = fixture(&account);
        Self {
            events: broadcast::channel(64).0,
            conversations,
            state: Mutex::new(State {
                sequence: messages.len() as u64,
                messages,
            }),
            account,
        }
    }
    fn title(&self, chat: &ChatId) -> String {
        self.conversations
            .iter()
            .find(|conversation| &conversation.id == chat)
            .map_or_else(
                || chat.id.clone(),
                |conversation| conversation.title.clone(),
            )
    }
}
impl State {
    /// Appends a message from the local user; time never precedes the existing history.
    fn push(
        &mut self,
        chat: ChatId,
        client_id: Option<String>,
        content: Vec<Content>,
        reply: Option<Box<Reply>>,
    ) -> Message {
        self.sequence += 1;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64);
        let latest = self
            .messages
            .last()
            .map_or(BASE_MS, |message| message.time_ms);
        let message = Message {
            id: MessageId {
                chat,
                id: self.sequence.to_string(),
            },
            client_id,
            sender: me(),
            time_ms: now.max(latest + 1),
            outgoing: true,
            content,
            reply,
            deleted: false,
        };
        self.messages.push(message.clone());
        message
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
                user: me(),
            })
        })
    }
    fn conversations(&self, page: PageRequest) -> Task<'_, Page<Conversation>> {
        Box::pin(async move {
            page.validate()?;
            let start = page
                .cursor
                .as_deref()
                .map(|cursor| {
                    cursor
                        .parse::<usize>()
                        .map_err(|_| Error::Invalid("Invalid cursor".into()))
                })
                .transpose()?
                .unwrap_or(0);
            let mut items = self.conversations.clone();
            {
                let state = self.state.lock().unwrap();
                for conversation in &mut items {
                    conversation.last_activity_ms = state
                        .messages
                        .iter()
                        .rev()
                        .find(|message| message.id.chat == conversation.id)
                        .map_or(0, |message| message.time_ms);
                }
            }
            items.sort_by_key(|conversation| std::cmp::Reverse(conversation.last_activity_ms));
            let end = start.saturating_add(page.limit as usize);
            Ok(Page {
                next: (end < items.len()).then(|| end.to_string()),
                items: items
                    .into_iter()
                    .skip(start)
                    .take(page.limit as usize)
                    .collect(),
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
            let state = self.state.lock().unwrap();
            let mut items: Vec<_> = state
                .messages
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
            let mut state = self.state.lock().unwrap();
            if let Some(existing) = state.messages.iter().find(|message| {
                message.id.chat == request.chat
                    && message.client_id.as_ref() == Some(&request.client_id)
            }) {
                return Ok(SendOutcome::confirmed(existing.clone()));
            }
            let reply = match &request.reply_to {
                Some(target) => {
                    let original = state
                        .messages
                        .iter()
                        .find(|message| &message.id == target)
                        .ok_or_else(|| Error::Invalid("Unknown reply target".into()))?;
                    Some(Box::new(Reply {
                        target: original.id.clone(),
                        sender: original.sender.clone(),
                        preview: original.content.clone(),
                    }))
                }
                None => None,
            };
            let message = state.push(
                request.chat,
                Some(request.client_id),
                request.content,
                reply,
            );
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
            let display = vec![DisplayPart::Text(format!(
                "You tapped {}",
                self.title(&chat)
            ))];
            let message = self.state.lock().unwrap().push(
                chat,
                None,
                vec![Content::Extension(ExtensionContent {
                    kind: "example.poke".into(),
                    version: 1,
                    data: request.data,
                    layout: Layout::Notice,
                    display,
                })],
                None,
            );
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
    async fn all_history(
        adapter: &dyn Adapter,
        chat: &ChatId,
        limit: u32,
    ) -> (Vec<Message>, usize) {
        let mut cursor = None;
        let mut items = Vec::new();
        let mut pages = 0;
        loop {
            let page = adapter
                .history(chat.clone(), PageRequest { cursor, limit })
                .await
                .unwrap();
            pages += 1;
            items.extend(page.items);
            match page.next {
                Some(next) => cursor = Some(next),
                None => return (items, pages),
            }
        }
    }
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
        assert_eq!(chat, adapter.account().chat("room"));
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
                    cursor: first.next.clone(),
                    limit: 1,
                },
            )
            .await
            .unwrap();
        assert!(
            second.items[0].id.id.parse::<u64>().unwrap()
                < first.items[0].id.id.parse::<u64>().unwrap()
        );
        let (history, _) = all_history(adapter.as_ref(), &chat, 1).await;
        assert_eq!(history[0], first.items[0]);
        assert_eq!(history[1], second.items[0]);
        assert_eq!(history.last().unwrap().content[0].text_preview(), "Hello");
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
    #[tokio::test]
    async fn fixture_has_ordered_conversations_and_pageable_histories_for_every_chat() {
        let adapter = MockAdapter::new("one");
        let mut conversations = Vec::new();
        let mut cursor = None;
        loop {
            let page = adapter
                .conversations(PageRequest { cursor, limit: 2 })
                .await
                .unwrap();
            conversations.extend(page.items);
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(conversations.len(), 5);
        assert!(
            conversations
                .windows(2)
                .all(|pair| pair[0].last_activity_ms >= pair[1].last_activity_ms)
        );
        assert!(
            conversations
                .iter()
                .any(|conversation| conversation.kind == ChatKind::Group)
        );
        assert!(
            conversations
                .iter()
                .any(|conversation| conversation.unread > 0)
        );
        for conversation in &conversations {
            let (history, pages) = all_history(&adapter, &conversation.id, 25).await;
            assert!(history.len() >= 4, "{}", conversation.title);
            assert!(
                history
                    .iter()
                    .all(|message| message.id.chat == conversation.id)
            );
            assert!(history.windows(2).all(|pair| {
                pair[0].id.id.parse::<u64>().unwrap() > pair[1].id.id.parse::<u64>().unwrap()
                    && pair[0].time_ms >= pair[1].time_ms
            }));
            assert_eq!(history[0].time_ms, conversation.last_activity_ms);
            assert!(
                history
                    .iter()
                    .all(|message| message.time_ms > 1_700_000_000_000)
            );
            assert!(history.iter().any(|message| message.outgoing));
            assert!(history.iter().any(|message| !message.outgoing));
            if conversation.id.id == "team" {
                assert!(history.len() >= 60);
                assert!(pages >= 3);
            }
            for message in &history {
                if let Some(reply) = &message.reply {
                    assert!(history.iter().any(|target| target.id == reply.target));
                }
            }
        }
        let group = conversations
            .iter()
            .find(|conversation| conversation.kind == ChatKind::Group)
            .unwrap();
        let (history, _) = all_history(&adapter, &group.id, 100).await;
        let senders: BTreeSet<_> = history
            .iter()
            .filter(|message| !message.outgoing)
            .map(|message| &message.sender.id)
            .collect();
        assert!(senders.len() >= 3);
    }
    #[tokio::test]
    async fn sends_replies_and_pokes_work_in_every_conversation() {
        let adapter = MockAdapter::new("one");
        let mut events = adapter.subscribe();
        let conversations = adapter
            .conversations(PageRequest::default())
            .await
            .unwrap()
            .items;
        for conversation in conversations {
            let latest = adapter
                .history(
                    conversation.id.clone(),
                    PageRequest {
                        cursor: None,
                        limit: 1,
                    },
                )
                .await
                .unwrap()
                .items
                .remove(0);
            let request = SendRequest {
                chat: conversation.id.clone(),
                client_id: format!("client-{}", conversation.id.id),
                content: vec![Content::Text("你好 👋".into())],
                reply_to: Some(latest.id.clone()),
            };
            let SendOutcome::Confirmed(sent) = adapter.send(request.clone()).await.unwrap() else {
                panic!()
            };
            assert!(sent.outgoing && sent.time_ms > latest.time_ms);
            assert_eq!(sent.reply.as_ref().unwrap().target, latest.id);
            let SendOutcome::Confirmed(again) = adapter.send(request).await.unwrap() else {
                panic!()
            };
            assert_eq!(again.id, sent.id);
            let Event::Message(event) = events.recv().await.unwrap() else {
                panic!()
            };
            assert_eq!(event, *sent);
            adapter
                .execute(ExtensionRequest {
                    action: "example.poke".into(),
                    chat: Some(conversation.id.clone()),
                    message: None,
                    user: None,
                    data: serde_json::Value::Null,
                })
                .await
                .unwrap();
            let Event::Message(poke) = events.recv().await.unwrap() else {
                panic!()
            };
            assert_eq!(
                poke.content[0].text_preview(),
                format!("You tapped {}", conversation.title)
            );
            let page = adapter
                .history(conversation.id.clone(), PageRequest::default())
                .await
                .unwrap();
            assert_eq!(page.items[0].id, poke.id);
            assert_eq!(page.items[1].id, sent.id);
        }
    }
}
