//! Matrix adapter built on the official Matrix Rust SDK.
#![forbid(unsafe_code)]
mod mapping;
use chat_adapter::*;
use matrix_sdk::{
    Client, Room,
    authentication::matrix::MatrixSession,
    config::SyncSettings,
    media::{MediaFormat, MediaRequestParameters},
    room::MessagesOptions,
    ruma::{
        self,
        api::client::session::get_login_types::v3::LoginType,
        events::{
            AnySyncTimelineEvent,
            room::{MediaSource, message::RoomMessageEventContent},
        },
        serde::Raw,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, broadcast, watch},
    task::JoinHandle,
};
#[derive(Serialize, Deserialize)]
struct Saved {
    homeserver: String,
    session: MatrixSession,
}
struct State {
    client: Option<Client>,
    homeserver: Option<String>,
    methods: Vec<LoginChoice>,
    sync: Option<JoinHandle<()>>,
    stop: Option<watch::Sender<bool>>,
    ready: bool,
    closed: bool,
}
pub struct MatrixAdapter {
    account: AccountId,
    store: Arc<dyn Store>,
    directory: PathBuf,
    device_name: String,
    events: broadcast::Sender<Event>,
    state: Mutex<State>,
}
fn failure(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}
fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
fn field(id: &str, label: &str, kind: FieldKind, required: bool) -> Field {
    Field {
        id: id.into(),
        label: label.into(),
        kind,
        required,
    }
}
fn homeserver_form() -> AuthState {
    AuthState::Form {
        step: "homeserver".into(),
        fields: vec![field("homeserver", "Homeserver", FieldKind::Text, true)],
    }
}
fn password_form() -> AuthState {
    AuthState::Form {
        step: "password".into(),
        fields: vec![
            field("username", "Username", FieldKind::Text, true),
            field("password", "Password", FieldKind::Secret, true),
        ],
    }
}
fn required<'a>(input: &'a AuthInput, key: &str) -> Result<&'a str> {
    input
        .fields
        .get(key)
        .map(String::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid(format!("Missing {key}")))
}
impl MatrixAdapter {
    pub fn new(account: impl Into<String>, store: Arc<dyn Store>, directory: PathBuf) -> Self {
        Self {
            account: AccountId {
                adapter: "matrix".into(),
                id: account.into(),
            },
            store,
            directory,
            device_name: "Matrix Rust SDK".into(),
            events: broadcast::channel(1024).0,
            state: Mutex::new(State {
                client: None,
                homeserver: None,
                methods: vec![],
                sync: None,
                stop: None,
                ready: false,
                closed: false,
            }),
        }
    }
    pub fn with_device_name(mut self, name: impl Into<String>) -> Self {
        self.device_name = name.into();
        self
    }
    fn key(&self, part: &str) -> String {
        format!("matrix/{}/{part}", hex::encode(self.account.id.as_bytes()))
    }
    async fn build(&self, homeserver: &str) -> Result<Client> {
        let url = matrix_sdk::reqwest::Url::parse(homeserver).map_err(failure)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(invalid("Invalid homeserver URL"));
        }
        tokio::fs::create_dir_all(&self.directory)
            .await
            .map_err(failure)?;
        Client::builder()
            .homeserver_url(url.as_str())
            .sqlite_store(&self.directory, None)
            .handle_refresh_tokens()
            .request_config(
                matrix_sdk::config::RequestConfig::new().timeout(Duration::from_secs(30)),
            )
            .build()
            .await
            .map_err(failure)
    }
    async fn choose(&self, state: &mut State, choice: &str) -> Result<AuthState> {
        if !state.methods.iter().any(|method| method.id == choice) {
            return Err(invalid("Unsupported login method"));
        }
        match choice {
            "password" => Ok(password_form()),
            "token" => Ok(AuthState::Form {
                step: "token".into(),
                fields: vec![field("token", "Login token", FieldKind::Secret, true)],
            }),
            "sso" => {
                let events = self.events.clone();
                state
                    .client
                    .as_ref()
                    .ok_or(Error::Unauthorized)?
                    .matrix_auth()
                    .login_sso(move |url| async move {
                        let _ = events.send(Event::Auth(AuthState::Browser { url }));
                        Ok(())
                    })
                    .initial_device_display_name(&self.device_name)
                    .request_refresh_token()
                    .await
                    .map_err(failure)?;
                self.ready(state).await
            }
            _ => Err(invalid("Unsupported login method")),
        }
    }
    async fn ready(&self, state: &mut State) -> Result<AuthState> {
        let client = state.client.as_ref().ok_or(Error::Unauthorized)?.clone();
        let identity = client.whoami().await.map_err(failure)?;
        let bound = self.store.get(self.key("user")).await?;
        if bound
            .as_ref()
            .is_some_and(|value| value.as_str() != Some(identity.user_id.as_str()))
        {
            return Err(invalid("This profile belongs to another Matrix account"));
        }
        self.store
            .put(
                self.key("user"),
                Value::String(identity.user_id.to_string()),
            )
            .await?;
        let name = client
            .account()
            .get_display_name()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| identity.user_id.localpart().into());
        let avatar = client
            .account()
            .get_avatar_url()
            .await
            .ok()
            .flatten()
            .map(|url| Box::new(mapping::avatar(url.as_str())));
        let user = User {
            id: identity.user_id.to_string(),
            name,
            avatar,
        };
        let session = client.matrix_auth().session().ok_or(Error::Unauthorized)?;
        self.store
            .put(
                self.key("session"),
                serde_json::to_value(Saved {
                    homeserver: client.homeserver().to_string(),
                    session,
                })
                .map_err(failure)?,
            )
            .await?;
        if state.sync.is_none() {
            let events = self.events.clone();
            let account = self.account.clone();
            let store = self.store.clone();
            let token_key = self.key("sync");
            let session_key = self.key("session");
            let event_account = account.clone();
            let event_events = events.clone();
            client.add_event_handler(move |raw: Raw<AnySyncTimelineEvent>, room: Room| {
                let account = event_account.clone();
                let events = event_events.clone();
                async move {
                    match serde_json::from_str(raw.json().get()) {
                        Ok(value) => match mapping::event(&account, &room, value).await {
                            Ok(Some(event)) => {
                                let _ = events.send(event);
                            }
                            Ok(None) => {}
                            Err(error) => {
                                let _ = events.send(Event::Error(error));
                            }
                        },
                        Err(error) => {
                            let _ = events.send(Event::Error(failure(error)));
                        }
                    }
                }
            });
            let (stop, mut stopped) = watch::channel(false);
            state.stop = Some(stop);
            state.sync = Some(tokio::spawn(async move {
                let mut settings = SyncSettings::default().timeout(Duration::from_secs(30));
                if let Ok(Some(Value::String(token))) = store.get(token_key.clone()).await {
                    settings = settings.token(token);
                }
                loop {
                    tokio::select! {
                        biased;
                        _=stopped.changed()=>break,
                        response=client.sync_once(settings.clone())=>match response {
                            Ok(response)=>{
                                let token=response.next_batch.to_string();settings=settings.token(token.clone());
                                if let Err(error)=store.put(token_key.clone(),Value::String(token)).await {let _=events.send(Event::Error(error));}
                                if let Some(session)=client.matrix_auth().session() {
                                    match serde_json::to_value(Saved {homeserver:client.homeserver().to_string(),session}) {
                                        Ok(value)=>if let Err(error)=store.put(session_key.clone(),value).await {let _=events.send(Event::Error(error));},
                                        Err(error)=>{let _=events.send(Event::Error(failure(error)));}
                                    }
                                }
                                for room in client.joined_rooms() {if let Ok(chat)=mapping::conversation(&account,&room).await {let _=events.send(Event::Conversation(chat));}}
                            }
                            Err(error)=>{
                                let _=events.send(Event::Error(failure(error)));
                                tokio::select! {_=tokio::time::sleep(Duration::from_secs(2))=>{},_=stopped.changed()=>break}
                            }
                        }
                    }
                }
            }));
        }
        state.ready = true;
        Ok(AuthState::Ready {
            account: self.account.clone(),
            user,
        })
    }
    async fn client(&self) -> Result<Client> {
        let state = self.state.lock().await;
        if !state.ready || state.closed {
            return Err(Error::Unauthorized);
        }
        state.client.clone().ok_or(Error::Unauthorized)
    }
    async fn room(&self, chat: &ChatId) -> Result<Room> {
        self.account.require_chat(chat)?;
        let id = ruma::RoomId::parse(&chat.id).map_err(failure)?;
        self.client()
            .await?
            .get_room(&id)
            .ok_or_else(|| invalid("Unknown Matrix room"))
    }
}
impl Adapter for MatrixAdapter {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            id: "matrix".into(),
            name: "Matrix".into(),
            capabilities: BTreeSet::from_iter(
                [
                    "core.text",
                    "core.reply",
                    "core.history",
                    "core.media.download",
                    "core.message.delete",
                    "matrix.reaction",
                    "matrix.e2ee",
                ]
                .map(String::from),
            ),
            actions: vec![
                Action {
                    id: "core.message.delete".into(),
                    label: "Delete".into(),
                    scope: Scope::Message,
                    fields: vec![],
                },
                Action {
                    id: "matrix.reaction".into(),
                    label: "React".into(),
                    scope: Scope::Message,
                    fields: vec![field("emoji", "Emoji", FieldKind::Text, true)],
                },
                Action {
                    id: "matrix.recover".into(),
                    label: "Recover encryption keys".into(),
                    scope: Scope::Account,
                    fields: vec![field(
                        "recovery_key",
                        "Recovery key",
                        FieldKind::Secret,
                        true,
                    )],
                },
            ],
        }
    }
    fn account(&self) -> AccountId {
        self.account.clone()
    }
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
    fn authenticate(&self, input: AuthInput) -> Task<'_, AuthState> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if state.closed {
                return Err(invalid("Session closed"));
            }
            if state.ready {
                return self.ready(&mut state).await;
            }
            match input.step.as_str() {
                "resume" => {
                    if let Some(value) = self.store.get(self.key("session")).await? {
                        let saved: Saved = serde_json::from_value(value).map_err(failure)?;
                        let client = self.build(&saved.homeserver).await?;
                        client
                            .restore_session(saved.session)
                            .await
                            .map_err(failure)?;
                        state.homeserver = Some(saved.homeserver);
                        state.client = Some(client);
                        self.ready(&mut state).await
                    } else {
                        Ok(homeserver_form())
                    }
                }
                "homeserver" => {
                    let homeserver = required(&input, "homeserver")?.trim();
                    let client = self.build(homeserver).await?;
                    let types = client
                        .matrix_auth()
                        .get_login_types()
                        .await
                        .map_err(failure)?;
                    state.methods = types
                        .flows
                        .iter()
                        .filter_map(|flow| match flow {
                            LoginType::Password(_) => Some(LoginChoice {
                                id: "password".into(),
                                label: "Password".into(),
                            }),
                            LoginType::Sso(_) => Some(LoginChoice {
                                id: "sso".into(),
                                label: "Single sign-on".into(),
                            }),
                            LoginType::Token(_) => Some(LoginChoice {
                                id: "token".into(),
                                label: "Login token".into(),
                            }),
                            _ => None,
                        })
                        .collect();
                    if state.methods.is_empty() {
                        return Err(Error::Unsupported(
                            "This homeserver does not offer a supported login method".into(),
                        ));
                    }
                    state.homeserver = Some(client.homeserver().to_string());
                    state.client = Some(client);
                    if state.methods.len() == 1 {
                        let method = state.methods[0].id.clone();
                        self.choose(&mut state, &method).await
                    } else {
                        Ok(AuthState::Choice {
                            step: "method".into(),
                            choices: state.methods.clone(),
                        })
                    }
                }
                "methods" => Ok(AuthState::Choice {
                    step: "method".into(),
                    choices: state.methods.clone(),
                }),
                "method" => self.choose(&mut state, required(&input, "choice")?).await,
                "password" => {
                    let client = state.client.as_ref().ok_or(Error::Unauthorized)?;
                    client
                        .matrix_auth()
                        .login_username(
                            required(&input, "username")?.trim(),
                            required(&input, "password")?,
                        )
                        .initial_device_display_name(&self.device_name)
                        .request_refresh_token()
                        .send()
                        .await
                        .map_err(failure)?;
                    self.ready(&mut state).await
                }
                "token" => {
                    state
                        .client
                        .as_ref()
                        .ok_or(Error::Unauthorized)?
                        .matrix_auth()
                        .login_token(required(&input, "token")?)
                        .initial_device_display_name(&self.device_name)
                        .request_refresh_token()
                        .send()
                        .await
                        .map_err(failure)?;
                    self.ready(&mut state).await
                }
                _ => Err(invalid("Unknown Matrix login step")),
            }
        })
    }
    fn conversations(&self, page: PageRequest) -> Task<'_, Page<Conversation>> {
        Box::pin(async move {
            page.validate()?;
            let client = self.client().await?;
            let mut rooms = client.joined_rooms();
            rooms.sort_by(|a, b| a.room_id().cmp(b.room_id()));
            let offset = page
                .cursor
                .map(|cursor| cursor.parse::<usize>().map_err(failure))
                .transpose()?
                .unwrap_or(0);
            let total = rooms.len();
            let mut items = vec![];
            for room in rooms.into_iter().skip(offset).take(page.limit as usize) {
                items.push(mapping::conversation(&self.account, &room).await?);
            }
            let end = offset + items.len();
            Ok(Page {
                items,
                next: (end < total).then(|| end.to_string()),
            })
        })
    }
    fn history(&self, chat: ChatId, page: PageRequest) -> Task<'_, Page<Message>> {
        Box::pin(async move {
            page.validate()?;
            let room = self.room(&chat).await?;
            let mut options = MessagesOptions::backward();
            options.from = page.cursor;
            options.limit = page.limit.into();
            let response = room.messages(options).await.map_err(failure)?;
            let mut items = vec![];
            for raw in response.chunk {
                let value = serde_json::from_str(raw.raw().json().get()).map_err(failure)?;
                if let Some(Event::Message(message)) =
                    mapping::event(&self.account, &room, value).await?
                {
                    items.push(message);
                }
            }
            Ok(Page {
                items,
                next: response.end,
            })
        })
    }
    fn send(&self, request: SendRequest) -> Task<'_, SendOutcome> {
        Box::pin(async move {
            let room = self.room(&request.chat).await?;
            let mut text = String::new();
            for part in &request.content {
                match part {
                    Content::Text(part) => text.push_str(part),
                    _ => return Err(Error::Unsupported("Non-text Matrix sending".into())),
                }
            }
            if text.trim().is_empty() {
                return Err(invalid("Empty message"));
            }
            let mut content = RoomMessageEventContent::text_plain(text);
            if let Some(reply) = &request.reply_to {
                if reply.chat != request.chat {
                    return Err(invalid("Reply belongs to another conversation"));
                }
                content.relates_to = Some(ruma::events::room::message::Relation::Reply(
                    ruma::events::relation::Reply::with_event_id(
                        ruma::EventId::parse(&reply.id).map_err(failure)?,
                    ),
                ));
            }
            let txn: ruma::OwnedTransactionId = request.client_id.clone().into();
            let response = room
                .send(content)
                .with_transaction_id(txn)
                .await
                .map_err(failure)?;
            let user = self
                .client()
                .await?
                .user_id()
                .ok_or(Error::Unauthorized)?
                .to_owned();
            let member = room.get_member_no_sync(&user).await.map_err(failure)?;
            Ok(SendOutcome::confirmed(Message {
                id: MessageId {
                    chat: request.chat,
                    id: response.response.event_id.to_string(),
                },
                client_id: Some(request.client_id),
                sender: User {
                    id: user.to_string(),
                    name: member
                        .as_ref()
                        .map_or(user.localpart(), |m| m.name())
                        .into(),
                    avatar: None,
                },
                time_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
                outgoing: true,
                content: request.content,
                reply: None,
                deleted: false,
            }))
        })
    }
    fn download(&self, media: Media, path: PathBuf) -> Task<'_, PathBuf> {
        Box::pin(async move {
            let source: MediaSource = serde_json::from_value(media.data).map_err(failure)?;
            let bytes = self
                .client()
                .await?
                .media()
                .get_media_content(
                    &MediaRequestParameters {
                        source,
                        format: MediaFormat::File,
                    },
                    true,
                )
                .await
                .map_err(failure)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(failure)?;
            }
            let temporary = path.with_extension("part");
            tokio::fs::write(&temporary, bytes).await.map_err(failure)?;
            tokio::fs::rename(&temporary, &path)
                .await
                .map_err(failure)?;
            Ok(path)
        })
    }
    fn execute(&self, request: ExtensionRequest) -> Task<'_, Value> {
        Box::pin(async move {
            if request.action == "matrix.recover" {
                let key = request
                    .data
                    .get("recovery_key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("Missing recovery key"))?;
                self.client()
                    .await?
                    .encryption()
                    .recovery()
                    .recover(key)
                    .await
                    .map_err(failure)?;
                return Ok(Value::Null);
            }
            let message = request.message.ok_or_else(|| invalid("Select a message"))?;
            let room = self.room(&message.chat).await?;
            let id = ruma::EventId::parse(&message.id).map_err(failure)?;
            match request.action.as_str() {
                "core.message.delete" => {
                    room.redact(&id, None, None).await.map_err(failure)?;
                    let _ = self.events.send(Event::Deleted(vec![message]));
                }
                "matrix.reaction" => {
                    let emoji = request
                        .data
                        .get("emoji")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid("Select an emoji"))?;
                    room.send(ruma::events::reaction::ReactionEventContent::new(
                        ruma::events::relation::Annotation::new(id, emoji.into()),
                    ))
                    .await
                    .map_err(failure)?;
                }
                _ => return Err(Error::Unsupported(request.action)),
            }
            Ok(json!({}))
        })
    }
    fn close(&self) -> Task<'_, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            state.closed = true;
            state.ready = false;
            if let Some(stop) = state.stop.take() {
                let _ = stop.send(true);
            }
            if let Some(task) = state.sync.take() {
                task.abort();
                let _ = task.await;
            }
            state.client = None;
            Ok(())
        })
    }
}
impl Drop for MatrixAdapter {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.try_lock() {
            if let Some(stop) = state.stop.take() {
                let _ = stop.send(true);
            }
            if let Some(task) = state.sync.take() {
                task.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests;
