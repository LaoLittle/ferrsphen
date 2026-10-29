//! A native Rust Telegram adapter.
#![forbid(unsafe_code)]
mod mapping;
mod session;
mod transport;
use chat_adapter::*;
use grammers_client::{
    Client,
    client::{LoginToken, PasswordToken, QrLogin, SignInError},
    message::InputMessage,
};
use grammers_mtsender::{
    ConnectionParams, InvocationError, SenderPool, SenderPoolFatHandle, UpdatesConfiguration,
};
use grammers_session::{
    Session,
    types::{PeerId, PeerRef},
};
use serde::{Deserialize, Serialize};
use session::StoredSession;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{Mutex, broadcast, watch},
    task::JoinHandle,
};
pub use transport::TdTransport;

#[derive(Clone)]
struct Credentials {
    api_id: i32,
    api_hash: String,
}
#[derive(Serialize, Deserialize)]
struct DialogCursor {
    date: i32,
    id: i32,
    peer: PeerRef,
}
struct Connection {
    client: Client,
    session: Arc<StoredSession>,
    handle: SenderPoolFatHandle,
    updates: Option<tokio::sync::mpsc::Receiver<grammers_session::updates::UpdatesLike>>,
    pool_task: JoinHandle<()>,
    update_task: Option<JoinHandle<()>>,
    relay_task: JoinHandle<()>,
    login_updates: watch::Receiver<u64>,
    stop: watch::Sender<bool>,
}
struct State {
    connection: Option<Connection>,
    credentials: Option<Credentials>,
    login: Option<LoginToken>,
    password: Option<PasswordToken>,
    ready: bool,
    closed: bool,
}
#[derive(Clone)]
pub struct Application {
    pub name: String,
    pub version: String,
    pub system_language: String,
}
impl Default for Application {
    fn default() -> Self {
        Self {
            name: "Rust Telegram client".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            system_language: "en".into(),
        }
    }
}
pub struct TelegramAdapter {
    account: AccountId,
    application: Application,
    configured_credentials: Option<Credentials>,
    store: Arc<dyn Store>,
    events: broadcast::Sender<Event>,
    state: Mutex<State>,
    known: Arc<Mutex<BTreeMap<MessageId, Message>>>,
}
fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
fn transport_error(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}
fn rpc(error: InvocationError) -> Error {
    match error {
        InvocationError::Rpc(error) => Error::Remote {
            code: error.name.clone(),
            message: error.to_string(),
            retry_after_seconds: if error.name == "FLOOD_WAIT" {
                error.value
            } else {
                None
            },
        },
        error => transport_error(error),
    }
}
fn field(id: &str, label: &str, kind: FieldKind) -> Field {
    Field {
        id: id.into(),
        label: label.into(),
        kind,
        required: true,
    }
}
fn form(step: &str, fields: Vec<Field>) -> AuthState {
    AuthState::Form {
        step: step.into(),
        fields,
    }
}
fn credentials_form() -> AuthState {
    form(
        "credentials",
        vec![
            field("api_id", "API ID", FieldKind::Integer),
            field("api_hash", "API Hash", FieldKind::Secret),
        ],
    )
}
fn phone_form() -> AuthState {
    form(
        "phone",
        vec![field("phone", "Phone number", FieldKind::Phone)],
    )
}
fn login_methods() -> AuthState {
    AuthState::Choice {
        step: "login_method".into(),
        choices: vec![
            LoginChoice {
                id: "qr".into(),
                label: "QR code".into(),
            },
            LoginChoice {
                id: "phone".into(),
                label: "Phone number".into(),
            },
        ],
    }
}
fn login_token_update(update: &grammers_session::updates::UpdatesLike) -> bool {
    use grammers_session::updates::UpdatesLike;
    use grammers_tl_types::enums::{Update, Updates};
    match update {
        UpdatesLike::Updates(Updates::UpdateShort(update)) => {
            matches!(update.update, Update::LoginToken)
        }
        UpdatesLike::Updates(Updates::Updates(updates)) => updates
            .updates
            .iter()
            .any(|update| matches!(update, Update::LoginToken)),
        UpdatesLike::Updates(Updates::Combined(updates)) => updates
            .updates
            .iter()
            .any(|update| matches!(update, Update::LoginToken)),
        _ => false,
    }
}
async fn relay_updates(
    mut updates: tokio::sync::mpsc::Receiver<grammers_session::updates::UpdatesLike>,
    forward: tokio::sync::mpsc::Sender<grammers_session::updates::UpdatesLike>,
    login_notify: watch::Sender<u64>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            biased;
            _=stop.changed()=>break,
            update=updates.recv()=>match update {
                Some(update)=>{
                    if login_token_update(&update) {login_notify.send_modify(|value|*value=value.wrapping_add(1));}
                    tokio::select! {biased;_=stop.changed()=>break,result=forward.send(update)=>if result.is_err(){break;}}
                },
                None=>break,
            }
        }
    }
}
fn qr_url(token: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "tg://login?token={}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token)
    )
}
fn qr_lifetime(expires: i32) -> Duration {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Duration::from_secs((i64::from(expires) - now as i64).clamp(1, 60) as u64)
}
fn code_form() -> AuthState {
    form(
        "code",
        vec![field("code", "Verification code", FieldKind::Code)],
    )
}
fn password_form() -> AuthState {
    form(
        "password",
        vec![field(
            "password",
            "Two-step verification password",
            FieldKind::Secret,
        )],
    )
}
fn required<'a>(input: &'a AuthInput, key: &str) -> Result<&'a str> {
    input
        .fields
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(format!("Missing {key}")))
}
impl TelegramAdapter {
    pub fn new(local_account: impl Into<String>, store: Arc<dyn Store>) -> Self {
        Self {
            account: AccountId {
                adapter: "telegram".into(),
                id: local_account.into(),
            },
            application: Application::default(),
            configured_credentials: None,
            store,
            events: broadcast::channel(1024).0,
            state: Mutex::new(State {
                connection: None,
                credentials: None,
                login: None,
                password: None,
                ready: false,
                closed: false,
            }),
            known: Arc::default(),
        }
    }
    pub fn with_application(mut self, application: Application) -> Self {
        self.application = application;
        self
    }
    /// Set application credentials for this adapter instance.
    pub fn with_credentials(mut self, api_id: i32, api_hash: impl Into<String>) -> Self {
        self.configured_credentials = Some(Credentials {
            api_id,
            api_hash: api_hash.into(),
        });
        self
    }
    fn prefix(&self) -> String {
        format!("telegram/{}", hex::encode(self.account.id.as_bytes()))
    }
    async fn connect(&self, state: &mut State, credentials: Credentials) -> Result<()> {
        if credentials.api_id <= 0
            || credentials.api_hash.len() != 32
            || !credentials
                .api_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid("Invalid Telegram API ID or API Hash"));
        }
        if let Some(mut old) = state.connection.take() {
            let _ = old.stop.send(true);
            if let Some(task) = old.update_task.take() {
                let _ = task.await;
            }
            let _ = old.relay_task.await;
            old.handle.quit();
            let _ = old.pool_task.await;
        }
        state.ready = false;
        state.login = None;
        state.password = None;
        let session = Arc::new(
            StoredSession::open(self.store.clone(), format!("{}/session", self.prefix())).await?,
        );
        let params = ConnectionParams {
            device_model: self.application.name.clone(),
            system_version: std::env::consts::OS.into(),
            app_version: self.application.version.clone(),
            system_lang_code: self.application.system_language.clone(),
            lang_code: self.application.system_language.clone(),
            transport: Box::new(|| Box::new(transport::TdTransport::default())),
            ..Default::default()
        };
        let SenderPool {
            runner,
            handle,
            updates,
        } = SenderPool::with_configuration(session.clone(), credentials.api_id, params);
        let client = Client::new(handle.clone());
        let pool_task = tokio::spawn(runner.run());
        let stop = watch::channel(false).0;
        let relay_stop = stop.subscribe();
        let (login_notify, login_updates) = watch::channel(0_u64);
        let (forward, forwarded) = tokio::sync::mpsc::channel(512);
        let relay_task = tokio::spawn(relay_updates(updates, forward, login_notify, relay_stop));
        state.connection = Some(Connection {
            client,
            session,
            handle,
            updates: Some(forwarded),
            pool_task,
            update_task: None,
            relay_task,
            login_updates,
            stop,
        });
        state.credentials = Some(credentials);
        Ok(())
    }
    async fn qr_login(&self, state: &mut State) -> Result<AuthState> {
        let connection = state.connection.as_ref().ok_or(Error::Unauthorized)?;
        let client = connection.client.clone();
        let mut updates = connection.login_updates.clone();
        let api_hash = state
            .credentials
            .as_ref()
            .ok_or(Error::Unauthorized)?
            .api_hash
            .clone();
        state.login = None;
        state.password = None;
        loop {
            updates.borrow_and_update();
            match client.request_qr_login(&api_hash).await {
                Ok(QrLogin::Token { expires, token }) => {
                    let _ = self.events.send(Event::Auth(AuthState::Qr {
                        url: qr_url(&token),
                        expires_ms: Some(i64::from(expires) * 1000),
                        choices: vec![LoginChoice {
                            id: "phone_login".into(),
                            label: "Phone number".into(),
                        }],
                    }));
                    tokio::select! {
                        _ = tokio::time::sleep(qr_lifetime(expires)) => {},
                        result = updates.changed() => if result.is_err() {return Err(transport_error("Telegram connection closed"));},
                    }
                }
                Ok(QrLogin::Authorized(_)) => return self.ready(state).await,
                Err(SignInError::PasswordRequired(token)) => {
                    state.password = Some(token);
                    return Ok(password_form());
                }
                Err(SignInError::Other(error)) => return Err(rpc(error)),
                Err(error) => return Err(transport_error(error)),
            }
        }
    }
    async fn bind_user(&self, user: &str) -> Result<()> {
        let key = format!("{}/user", self.prefix());
        if let Some(value) = self.store.get(key.clone()).await? {
            if value.as_str() != Some(user) {
                return Err(invalid("This profile belongs to another Telegram account"));
            }
        } else {
            self.store
                .put(key, serde_json::Value::String(user.into()))
                .await?;
        }
        Ok(())
    }
    async fn ready(&self, state: &mut State) -> Result<AuthState> {
        let connection = state.connection.as_mut().ok_or(Error::Unauthorized)?;
        let user = connection.client.get_me().await.map_err(rpc)?;
        self.bind_user(&user.id().bot_api_dialog_id_unchecked().to_string())
            .await?;
        let auth = AuthState::Ready {
            account: self.account.clone(),
            user: mapping::user(&user),
        };
        connection.session.persist().await?;
        if let Some(updates) = connection.updates.take() {
            let client = connection.client.clone();
            let events = self.events.clone();
            let account = self.account.clone();
            let known = self.known.clone();
            let mut stop = connection.stop.subscribe();
            connection.update_task = Some(tokio::spawn(async move {
                let mut stream = match client
                    .stream_updates(updates, UpdatesConfiguration { catch_up: true })
                    .await
                {
                    Ok(stream) => stream,
                    Err(error) => {
                        let _ = events.send(Event::Error(transport_error(error)));
                        return;
                    }
                };
                let mut checkpoint = tokio::time::interval(Duration::from_secs(15));
                loop {
                    tokio::select! {
                        biased;
                        _=stop.changed()=>break,
                        _=checkpoint.tick()=>if let Err(error)=stream.sync_update_state().await {let _=events.send(Event::Error(transport_error(error)));},
                        update=stream.next()=>match update {
                            Ok(grammers_client::update::Update::NewMessage(message) | grammers_client::update::Update::MessageEdited(message))=>{
                                let mapped=mapping::message(&account,&message);
                                known.lock().await.insert(mapped.id.clone(),mapped.clone());
                                let _=events.send(Event::Message(mapped));
                            }
                            Ok(grammers_client::update::Update::MessageDeleted(deletion))=>{
                                let channel=deletion.channel_id().map(|id| PeerId::channel_unchecked(id).bot_api_dialog_id_unchecked().to_string());
                                let ids:Vec<_>=known.lock().await.keys().filter(|key| {
                                    let correct_chat=if let Some(channel)=&channel { &key.chat.id==channel } else {
                                        key.chat.id.parse::<i64>().ok().and_then(PeerId::from_bot_api_dialog_id).is_some_and(|id|id.kind()!=grammers_session::types::PeerKind::Channel)
                                    };
                                    correct_chat && key.id.parse::<i32>().ok().is_some_and(|id|deletion.messages().contains(&id))
                                }).cloned().collect();
                                let _=events.send(Event::Deleted(ids));
                            }
                            Ok(_)=>{},
                            Err(error)=>{
                                let _=events.send(Event::Error(rpc(error)));let _=events.send(Event::ResyncRequired);
                                tokio::select! {_=tokio::time::sleep(Duration::from_secs(1))=>{},_=stop.changed()=>break}
                            }
                        }
                    }
                }
                if let Err(error) = stream.sync_update_state().await {
                    let _ = events.send(Event::Error(transport_error(error)));
                }
            }));
        }
        state.ready = true;
        Ok(auth)
    }
    async fn client(&self) -> Result<(Client, Arc<StoredSession>)> {
        let state = self.state.lock().await;
        if !state.ready || state.closed {
            return Err(Error::Unauthorized);
        }
        let connection = state.connection.as_ref().ok_or(Error::Unauthorized)?;
        Ok((connection.client.clone(), connection.session.clone()))
    }
    async fn peer(&self, chat: &ChatId) -> Result<PeerRef> {
        self.account.require_chat(chat)?;
        let (_, session) = self.client().await?;
        let id = chat
            .id
            .parse()
            .ok()
            .and_then(PeerId::from_bot_api_dialog_id)
            .ok_or_else(|| invalid("Invalid Telegram peer"))?;
        session
            .peer_ref(id)
            .await?
            .ok_or_else(|| invalid("Unknown Telegram peer; load conversations first"))
    }
}
impl Adapter for TelegramAdapter {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            id: "telegram".into(),
            name: "Telegram".into(),
            capabilities: BTreeSet::from_iter(
                [
                    "core.text",
                    "core.reply",
                    "core.history",
                    "core.media.download",
                    "telegram.reaction",
                    "core.message.delete",
                ]
                .map(String::from),
            ),
            actions: vec![
                Action {
                    id: "telegram.reaction".into(),
                    label: "React".into(),
                    scope: Scope::Message,
                    fields: vec![field("emoji", "Emoji", FieldKind::Text)],
                },
                Action {
                    id: "core.message.delete".into(),
                    label: "Delete".into(),
                    scope: Scope::Message,
                    fields: vec![],
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
            if state.ready && input.step != "resume" {
                return Err(invalid("Already signed in"));
            }
            let result = match input.step.as_str() {
                "resume" => {
                    self.store
                        .delete(format!("{}/credentials", self.prefix()))
                        .await?;
                    if state.connection.is_none() {
                        if let Some(credentials) = self.configured_credentials.clone() {
                            self.connect(&mut state, credentials).await?;
                        } else {
                            return Ok(credentials_form());
                        }
                    }
                    if state
                        .connection
                        .as_ref()
                        .unwrap()
                        .client
                        .is_authorized()
                        .await
                        .map_err(rpc)?
                    {
                        self.ready(&mut state).await?
                    } else {
                        login_methods()
                    }
                }
                "credentials" => {
                    let credentials = Credentials {
                        api_id: required(&input, "api_id")?
                            .trim()
                            .parse()
                            .map_err(|_| invalid("Invalid API ID"))?,
                        api_hash: required(&input, "api_hash")?.trim().into(),
                    };
                    self.connect(&mut state, credentials).await?;

                    if state
                        .connection
                        .as_ref()
                        .unwrap()
                        .client
                        .is_authorized()
                        .await
                        .map_err(rpc)?
                    {
                        self.ready(&mut state).await?
                    } else {
                        login_methods()
                    }
                }
                "login_method" => match required(&input, "choice")? {
                    "qr" => self.qr_login(&mut state).await?,
                    "phone" => phone_form(),
                    _ => return Err(invalid("Unknown login method")),
                },
                "phone_login" => phone_form(),
                "phone" => {
                    let client = state
                        .connection
                        .as_ref()
                        .ok_or(Error::Unauthorized)?
                        .client
                        .clone();
                    let hash = &state
                        .credentials
                        .as_ref()
                        .ok_or(Error::Unauthorized)?
                        .api_hash;
                    state.login = Some(
                        client
                            .request_login_code(required(&input, "phone")?.trim(), hash)
                            .await
                            .map_err(rpc)?,
                    );
                    code_form()
                }
                "code" => {
                    let client = state
                        .connection
                        .as_ref()
                        .ok_or(Error::Unauthorized)?
                        .client
                        .clone();
                    let token = state
                        .login
                        .as_ref()
                        .ok_or_else(|| invalid("Request a verification code first"))?;
                    match client
                        .sign_in(token, required(&input, "code")?.trim())
                        .await
                    {
                        Ok(_) => {
                            state.login = None;
                            self.ready(&mut state).await?
                        }
                        Err(SignInError::PasswordRequired(token)) => {
                            state.password = Some(token);
                            password_form()
                        }
                        Err(error) => return Err(transport_error(error)),
                    }
                }
                "password" => {
                    let client = state
                        .connection
                        .as_ref()
                        .ok_or(Error::Unauthorized)?
                        .client
                        .clone();
                    let password = required(&input, "password")?;
                    let token = state
                        .password
                        .take()
                        .ok_or_else(|| invalid("Two-step verification is not pending"))?;
                    match client.check_password(token, password).await {
                        Ok(_) => self.ready(&mut state).await?,
                        Err(SignInError::InvalidPassword(token)) => {
                            state.password = Some(token);
                            return Err(invalid("Invalid password"));
                        }
                        Err(error) => return Err(transport_error(error)),
                    }
                }
                _ => return Err(invalid("Unknown authentication step")),
            };
            let _ = self.events.send(Event::Auth(result.clone()));
            Ok(result)
        })
    }
    fn conversations(&self, page: PageRequest) -> Task<'_, Page<Conversation>> {
        Box::pin(async move {
            page.validate()?;
            let (client, _) = self.client().await?;
            let mut iterator = client.iter_dialogs().limit(page.limit as usize + 1);
            if let Some(cursor) = page.cursor {
                let cursor: DialogCursor = serde_json::from_str(&cursor)
                    .map_err(|_| invalid("Invalid conversation cursor"))?;
                iterator = iterator.offset(cursor.date, cursor.id, cursor.peer);
            }
            let mut dialogs = Vec::new();
            while let Some(dialog) = iterator.next().await.map_err(rpc)? {
                dialogs.push(dialog);
            }
            let more = dialogs.len() > page.limit as usize;
            if more {
                dialogs.pop();
            }
            let next = if more {
                dialogs
                    .last()
                    .map(|dialog| {
                        serde_json::to_string(&DialogCursor {
                            date: dialog
                                .last_message
                                .as_ref()
                                .map_or(0, |m| m.date().as_second() as i32),
                            id: dialog.last_message.as_ref().map_or(0, |m| m.id()),
                            peer: dialog.peer_ref(),
                        })
                    })
                    .transpose()
                    .map_err(transport_error)?
            } else {
                None
            };
            let items = dialogs
                .iter()
                .map(|dialog| mapping::conversation(&self.account, dialog))
                .collect();
            Ok(Page { items, next })
        })
    }
    fn history(&self, chat: ChatId, page: PageRequest) -> Task<'_, Page<Message>> {
        Box::pin(async move {
            page.validate()?;
            let (client, _) = self.client().await?;
            let peer = self.peer(&chat).await?;
            let mut iterator = client.iter_messages(peer).limit(page.limit as usize + 1);
            if let Some(cursor) = page.cursor {
                iterator = iterator.offset_id(
                    cursor
                        .parse()
                        .map_err(|_| invalid("Invalid history cursor"))?,
                );
            }
            let mut messages = Vec::new();
            while let Some(message) = iterator.next().await.map_err(rpc)? {
                messages.push(message);
            }
            let more = messages.len() > page.limit as usize;
            if more {
                messages.pop();
            }
            let next = if more {
                messages.last().map(|message| message.id().to_string())
            } else {
                None
            };
            let items: Vec<_> = messages
                .iter()
                .map(|message| mapping::message(&self.account, message))
                .collect();
            let mut known = self.known.lock().await;
            for message in &items {
                known.insert(message.id.clone(), message.clone());
            }
            Ok(Page { items, next })
        })
    }
    fn send(&self, request: SendRequest) -> Task<'_, SendOutcome> {
        Box::pin(async move {
            let (client, _) = self.client().await?;
            let peer = self.peer(&request.chat).await?;
            let mut text = String::new();
            for content in &request.content {
                match content {
                    Content::Text(part) => text.push_str(part),
                    _ => return Err(Error::Unsupported("Non-text Telegram sending".into())),
                }
            }
            if text.is_empty() {
                return Err(invalid("Empty message"));
            }
            let bytes = sha1_smol::Sha1::from(request.client_id.as_bytes())
                .digest()
                .bytes();
            let random_id = (i64::from_le_bytes(bytes[..8].try_into().unwrap()) & i64::MAX).max(1);
            let mut input = InputMessage::new().text(text).random_id(random_id);
            if let Some(reply) = request.reply_to {
                if reply.chat != request.chat {
                    return Err(invalid("Reply belongs to another conversation"));
                }
                input = input.reply_to(Some(
                    reply.id.parse().map_err(|_| invalid("Invalid reply ID"))?,
                ));
            }
            let message = client
                .send_message(peer, input)
                .await
                .map_err(|error| match error {
                    InvocationError::Rpc(_) => rpc(error),
                    error => Error::Unconfirmed(error.to_string()),
                })?;
            let mut mapped = mapping::message(&self.account, &message);
            mapped.client_id = Some(request.client_id);
            mapped.outgoing = true;
            self.known
                .lock()
                .await
                .insert(mapped.id.clone(), mapped.clone());
            Ok(SendOutcome::confirmed(mapped))
        })
    }
    fn download(&self, media: Media, destination: PathBuf) -> Task<'_, PathBuf> {
        Box::pin(async move {
            let (client, _) = self.client().await?;
            let chat = self.account.chat(
                media
                    .data
                    .get("chat")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| invalid("Missing media peer"))?,
            );
            let peer = self.peer(&chat).await?;
            if let Some(parent) = destination.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(transport_error)?;
            }
            let temporary = destination.with_extension("part");
            if media.data.get("avatar").and_then(|v| v.as_bool()) == Some(true) {
                let peer = client.resolve_peer(peer).await.map_err(rpc)?;
                let photo = peer
                    .photo(false)
                    .await
                    .map_err(transport_error)?
                    .ok_or_else(|| invalid("No avatar"))?;
                client
                    .download_media(&photo, &temporary)
                    .await
                    .map_err(rpc)?;
            } else {
                let id = media
                    .data
                    .get("message")
                    .and_then(|v| v.as_i64())
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or_else(|| invalid("Missing media message"))?;
                let message = client
                    .get_messages_by_id(peer, &[id])
                    .await
                    .map_err(rpc)?
                    .into_iter()
                    .flatten()
                    .next()
                    .ok_or_else(|| invalid("Media message no longer exists"))?;
                if !message.download_media(&temporary).await.map_err(rpc)? {
                    return Err(invalid("Message has no downloadable media"));
                }
            }
            tokio::fs::rename(&temporary, &destination)
                .await
                .map_err(transport_error)?;
            Ok(destination)
        })
    }
    fn execute(&self, request: ExtensionRequest) -> Task<'_, serde_json::Value> {
        Box::pin(async move {
            let message = request.message.ok_or_else(|| invalid("Select a message"))?;
            let (client, _) = self.client().await?;
            let peer = self.peer(&message.chat).await?;
            let id = message
                .id
                .parse::<i32>()
                .map_err(|_| invalid("Invalid message ID"))?;
            match request.action.as_str() {
                "core.message.delete" => {
                    client.delete_messages(peer, &[id]).await.map_err(rpc)?;
                    let _ = self.events.send(Event::Deleted(vec![message]));
                }
                "telegram.reaction" => {
                    let emoji = request
                        .data
                        .get("emoji")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| invalid("Select an emoji"))?;
                    client.send_reactions(peer, id, emoji).await.map_err(rpc)?;
                }
                _ => return Err(Error::Unsupported(request.action)),
            }
            Ok(serde_json::Value::Null)
        })
    }
    fn close(&self) -> Task<'_, ()> {
        Box::pin(async move {
            let connection = {
                let mut state = self.state.lock().await;
                state.closed = true;
                state.ready = false;
                state.connection.take()
            };
            if let Some(mut connection) = connection {
                let _ = connection.stop.send(true);
                if let Some(task) = connection.update_task.take() {
                    let _ = task.await;
                }
                let _ = connection.relay_task.await;
                let result = connection.session.persist().await;
                connection.handle.quit();
                let _ = connection.pool_task.await;
                result?;
            }
            Ok(())
        })
    }
}
impl Drop for TelegramAdapter {
    fn drop(&mut self) {
        if let Ok(state) = self.state.try_lock()
            && let Some(connection) = &state.connection
        {
            let _ = connection.stop.send(true);
            connection.handle.quit();
        }
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    #[tokio::test]
    async fn cached_application_credentials_are_deleted_and_never_used_for_login() {
        let store: Arc<dyn Store> = Arc::new(chat_adapter::mock::MemoryStore::default());
        let adapter = TelegramAdapter::new("one", store.clone());
        let key = format!("{}/credentials", adapter.prefix());
        store
            .put(
                key.clone(),
                serde_json::json!({"api_id":12345,"api_hash":"synthetic-old-hash"}),
            )
            .await
            .unwrap();
        let auth = adapter
            .authenticate(AuthInput {
                step: "resume".into(),
                fields: Default::default(),
            })
            .await
            .unwrap();
        assert!(matches!(auth,AuthState::Form {step,..} if step=="credentials"));
        assert!(store.get(key).await.unwrap().is_none());
        let configured = TelegramAdapter::new("two", store.clone())
            .with_credentials(12345, "0123456789abcdef0123456789abcdef");
        assert!(
            store
                .get(format!("{}/credentials", configured.prefix()))
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn missing_application_configuration_requests_credentials_in_the_login_form() {
        let adapter = TelegramAdapter::new(
            "unconfigured",
            Arc::new(chat_adapter::mock::MemoryStore::default()),
        );
        assert!(adapter.configured_credentials.is_none());
        let AuthState::Form { step, fields } = adapter
            .authenticate(AuthInput {
                step: "resume".into(),
                fields: Default::default(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(step, "credentials");
        assert!(fields.iter().any(|field| field.id == "api_id"));
        assert!(
            fields
                .iter()
                .any(|field| field.id == "api_hash" && field.kind == FieldKind::Secret)
        );
    }
    #[tokio::test]
    async fn a_profile_cannot_silently_reuse_another_accounts_history() {
        let store: Arc<dyn Store> = Arc::new(chat_adapter::mock::MemoryStore::default());
        let first = TelegramAdapter::new("one", store.clone());
        first.bind_user("100").await.unwrap();
        first.bind_user("100").await.unwrap();
        assert!(first.bind_user("200").await.is_err());
        let reopened = TelegramAdapter::new("one", store.clone());
        assert!(reopened.bind_user("200").await.is_err());
        TelegramAdapter::new("two", store)
            .bind_user("200")
            .await
            .unwrap();
    }
}

#[cfg(test)]
mod qr_tests {
    use super::*;
    use grammers_session::updates::UpdatesLike;
    use grammers_tl_types::{
        enums::{Update, Updates},
        types::UpdateShort,
    };
    fn update(kind: Update) -> UpdatesLike {
        UpdatesLike::Updates(Updates::UpdateShort(UpdateShort {
            update: kind,
            date: 1,
        }))
    }
    #[test]
    fn login_uri_uses_unpadded_url_safe_base64_and_methods_include_phone_and_qr() {
        assert_eq!(qr_url(&[0xfb, 0xff]), "tg://login?token=-_8");
        let AuthState::Choice { choices, .. } = login_methods() else {
            panic!()
        };
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.id.as_str())
                .collect::<Vec<_>>(),
            ["qr", "phone"]
        );
        assert_eq!(qr_lifetime(0), Duration::from_secs(1));
        assert!(qr_lifetime(i32::MAX) <= Duration::from_secs(60));
    }
    #[tokio::test]
    async fn qr_login_notifications_do_not_consume_chat_updates_and_shutdown_unblocks_full_queue() {
        let (input, updates) = tokio::sync::mpsc::channel(4);
        let (forward, mut received) = tokio::sync::mpsc::channel(1);
        let (notify, mut notified) = watch::channel(0);
        let (stop, stopped) = watch::channel(false);
        let relay = tokio::spawn(relay_updates(updates, forward, notify, stopped));
        input.send(update(Update::Config)).await.unwrap();
        input.send(update(Update::LoginToken)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), notified.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*notified.borrow_and_update(), 1);
        assert!(!login_token_update(&received.recv().await.unwrap()));
        assert!(login_token_update(&received.recv().await.unwrap()));
        // A blocked forwarding queue must not keep a cancelled login/account alive.
        input.send(update(Update::Config)).await.unwrap();
        input.send(update(Update::Config)).await.unwrap();
        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), relay)
            .await
            .unwrap()
            .unwrap();
    }
}
