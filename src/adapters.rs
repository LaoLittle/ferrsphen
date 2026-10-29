//! Host-side orchestration shared by all protocol adapters.
use crate::storage::Database;
use chat_adapter::*;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};
use tokio::sync::{broadcast, mpsc, watch};

pub enum Command {
    Authenticate(AuthInput),
    CancelLogin,
    SavePreferences(crate::settings::Preferences),
    Conversations(PageRequest),
    History(ChatId, PageRequest),
    Send(SendRequest),
    Download(Media),
    Execute(ExtensionRequest),
    Close,
}
pub enum Update {
    Descriptor(Descriptor),
    Preferences(crate::settings::Preferences),
    Auth(AuthState),
    Login(LoginSnapshot),
    AuthResult {
        generation: u64,
        result: Result<AuthState>,
    },
    Event(Event),
    Conversations {
        page: Page<Conversation>,
        append: bool,
    },
    History {
        chat: ChatId,
        page: Page<Message>,
        cached: bool,
    },
    Sent {
        client_id: String,
        result: Result<SendOutcome>,
    },
    Downloaded {
        id: String,
        result: Result<PathBuf>,
    },
    Error(Error),
}
pub type Factory = Box<dyn FnOnce(Arc<dyn Store>) -> Arc<dyn Adapter> + Send>;
pub struct Backend {
    pub commands: mpsc::UnboundedSender<Command>,
    pub updates: Option<mpsc::UnboundedReceiver<Update>>,
    pub stopped: watch::Receiver<bool>,
}
impl Backend {
    pub fn start(root: PathBuf, factory: Factory) -> Self {
        let (finished, stopped) = watch::channel(false);
        let (commands, mut requests) = mpsc::unbounded_channel();
        let (send, updates) = mpsc::unbounded_channel();
        thread::Builder::new().name("chat-adapters".into()).spawn(move|| {
            let runtime=tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("adapter runtime");
            runtime.block_on(async move {
                let db=match Database::open(&root) {Ok(db)=>Arc::new(db),Err(error)=>{let _=send.send(Update::Error(Error::Transport(format!("{error:#}"))));return;}};
                if let Err(error)=db.purge_telegram_api_credentials().await {let _=send.send(Update::Error(db_error(error)));return;}
                match db.get(crate::settings::STORAGE_KEY) {Ok(preferences)=>{let _=send.send(Update::Preferences(preferences.unwrap_or_default()));},Err(error)=>{let _=send.send(Update::Error(db_error(error)));}}
                let store:Arc<dyn Store>=Arc::new(DbStore {db:db.clone()});
                let adapter=factory(store);let account=adapter.account();let mut events=adapter.subscribe();
                let mut ready=false;let mut jobs=tokio::task::JoinSet::new();
                let _=send.send(Update::Descriptor(adapter.descriptor()));
                let mut login=LoginMachine::default();let generation=login.begin();let _=send.send(Update::Login(login.snapshot()));
                let a=adapter.clone();let mut auth_job=Some(jobs.spawn(async move {Update::AuthResult {generation,result:a.authenticate(AuthInput {step:"resume".into(),fields:Default::default()}).await}}));
                loop {
                    tokio::select! {
                        request=requests.recv()=>match request {
                            Some(Command::Close)|None=>break,
                            Some(Command::SavePreferences(preferences))=>{if let Err(error)=db.put(crate::settings::STORAGE_KEY,&preferences).await {let _=send.send(Update::Error(db_error(error)));}},
                            Some(Command::Authenticate(input))=>{
                                if let Some(job)=auth_job.take(){job.abort();}let generation=login.begin();let _=send.send(Update::Login(login.snapshot()));
                                let a=adapter.clone();auth_job=Some(jobs.spawn(async move {Update::AuthResult {generation,result:a.authenticate(input).await}}));
                            },
                            Some(Command::CancelLogin)=>{if let Some(job)=auth_job.take(){job.abort();}login.cancel();let _=send.send(Update::Login(login.snapshot()));},
                            Some(Command::Conversations(page))=>{let a=adapter.clone();let append=page.cursor.is_some();jobs.spawn(async move {match a.conversations(page).await {Ok(page)=>Update::Conversations {page,append},Err(error)=>Update::Error(error)}});},
                            Some(Command::History(chat,page))=>{
                                if page.cursor.is_none() {match cached_messages(&db,&chat,100) {Ok(items)=>{let _=send.send(Update::History {chat:chat.clone(),page:Page {items,next:None},cached:true});},Err(error)=>{let _=send.send(Update::Error(error));}}}
                                let a=adapter.clone();jobs.spawn(async move {match a.history(chat.clone(),page).await {Ok(page)=>Update::History {chat,page,cached:false},Err(error)=>Update::Error(error)}});
                            },
                            Some(Command::Send(request))=>{let a=adapter.clone();jobs.spawn(async move {let client_id=request.client_id.clone();Update::Sent {client_id,result:a.send(request).await}});},
                            Some(Command::Download(media))=>{
                                let a=adapter.clone();let directory=root.join("cache/images");jobs.spawn(async move {
                                    let id=media.id.clone();let extension=match media.mime.as_deref(){Some("image/jpeg")=>"jpg",Some("image/png")=>"png",Some("image/gif")=>"gif",Some("image/webp")=>"webp",_=>"bin"};
                                    let path=directory.join(format!("{}.{}",sha1_smol::Sha1::from(id.as_bytes()).digest(),extension));
                                    let result=if path.is_file(){Ok(path)}else{a.download(media,path).await};Update::Downloaded {id,result}
                                });
                            },
                            Some(Command::Execute(request))=>{let a=adapter.clone();jobs.spawn(async move {match a.execute(request).await {Ok(_)=>Update::Event(Event::ResyncRequired),Err(error)=>Update::Error(error)}});},
                        },
                        event=events.recv()=>match event {
                            Ok(Event::Auth(auth))=>{login.observe(auth.clone());let _=send.send(Update::Login(login.snapshot()));if let Err(error)=publish(&db,&send,Update::Auth(auth),&account,&adapter,&mut jobs,&mut ready).await {let _=send.send(Update::Error(error));}},
                            Ok(event)=>match prepare_event(&db,&account,event) {
                                Ok(event)=>{if let Err(error)=persist_event(&db,&event).await {let _=send.send(Update::Error(error));}let _=send.send(Update::Event(event));},
                                Err(error)=>{let _=send.send(Update::Error(error));}
                            },
                            Err(broadcast::error::RecvError::Lagged(_))=>{let _=send.send(Update::Event(Event::ResyncRequired));},
                            Err(broadcast::error::RecvError::Closed)=>break,
                        },
                        result=jobs.join_next(),if !jobs.is_empty()=>if let Some(result)=result {
                            let update=match result {Ok(update)=>update,Err(error) if error.is_cancelled()=>continue,Err(error)=>Update::Error(Error::Transport(error.to_string()))};
                            let update=if let Update::AuthResult {generation,result}=update {
                                if !login.finish(generation,result.clone()){continue;}
                                auth_job=None;let _=send.send(Update::Login(login.snapshot()));
                                match result {Ok(auth)=>Update::Auth(auth),Err(error)=>Update::Error(error)}
                            }else{update};
                            if let Err(error)=publish(&db,&send,update,&account,&adapter,&mut jobs,&mut ready).await {let _=send.send(Update::Error(error));}
                        },
                    }
                }
                jobs.abort_all();while jobs.join_next().await.is_some(){}
                if let Err(error)=adapter.close().await {tracing::error!(%error,"adapter shutdown failed");}
                drop(adapter);
                if let Ok(db)=Arc::try_unwrap(db) && let Err(error)=db.close().await {tracing::error!(%error,"adapter database close failed");}
            });
            let _=finished.send(true);
        }).expect("adapter thread");
        Self {
            commands,
            updates: Some(updates),
            stopped,
        }
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Close);
    }
}
struct DbStore {
    db: Arc<Database>,
}
impl Store for DbStore {
    fn get(&self, key: String) -> Task<'_, Option<serde_json::Value>> {
        Box::pin(async move {
            self.db
                .get(&format!("adapter/session/{}", hex::encode(key)))
                .map_err(db_error)
        })
    }
    fn put(&self, key: String, value: serde_json::Value) -> Task<'_, ()> {
        Box::pin(async move {
            self.db
                .put(&format!("adapter/session/{}", hex::encode(key)), &value)
                .await
                .map_err(db_error)
        })
    }
    fn delete(&self, key: String) -> Task<'_, ()> {
        Box::pin(async move {
            self.db
                .delete(&format!("adapter/session/{}", hex::encode(key)))
                .await
                .map_err(db_error)
        })
    }
}
fn db_error(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}
fn record_key<T: serde::Serialize>(id: &T) -> String {
    hex::encode(serde_json::to_vec(id).expect("adapter identifier"))
}
fn conversation_key(account: &AccountId) -> String {
    format!("adapter/conversations/{}", record_key(account))
}
fn message_prefix(chat: &ChatId) -> String {
    format!("adapter/message/{}/", record_key(chat))
}
async fn save_message(db: &Database, message: &Message) -> Result<()> {
    db.put(
        &format!(
            "{}{:020}/{}",
            message_prefix(&message.id.chat),
            message.time_ms,
            hex::encode(&message.id.id)
        ),
        message,
    )
    .await
    .map_err(db_error)
}
fn cached_messages(db: &Database, chat: &ChatId, limit: usize) -> Result<Vec<Message>> {
    let mut messages: Vec<Message> = db
        .records_with_prefix(&message_prefix(chat), limit)
        .map_err(db_error)?;
    for message in &mut messages {
        if db
            .get::<bool>(&format!("adapter/deleted/{}", record_key(&message.id)))
            .map_err(db_error)?
            .unwrap_or(false)
        {
            message.deleted = true;
        }
    }
    Ok(messages)
}
fn prepare_message(db: &Database, account: &AccountId, message: &mut Message) -> Result<()> {
    account.require_chat(&message.id.chat)?;
    if db
        .get::<bool>(&format!("adapter/deleted/{}", record_key(&message.id)))
        .map_err(db_error)?
        .unwrap_or(false)
    {
        message.deleted = true;
    }
    Ok(())
}
fn prepare_event(db: &Database, account: &AccountId, mut event: Event) -> Result<Event> {
    match &mut event {
        Event::Message(message) => prepare_message(db, account, message)?,
        Event::Conversation(chat) => account.require_chat(&chat.id)?,
        Event::Deleted(ids) => {
            for id in ids {
                account.require_chat(&id.chat)?;
            }
        }
        Event::Auth(AuthState::Ready {
            account: actual, ..
        }) if actual != account => return Err(Error::Invalid("Account mismatch".into())),
        _ => {}
    }
    Ok(event)
}
async fn persist_event(db: &Database, event: &Event) -> Result<()> {
    match event {
        Event::Message(message) => save_message(db, message).await?,
        Event::Deleted(ids) => {
            for id in ids {
                db.put(&format!("adapter/deleted/{}", record_key(id)), &true)
                    .await
                    .map_err(db_error)?;
            }
        }
        _ => {}
    }
    Ok(())
}
async fn publish(
    db: &Database,
    send: &mpsc::UnboundedSender<Update>,
    mut update: Update,
    account: &AccountId,
    adapter: &Arc<dyn Adapter>,
    jobs: &mut tokio::task::JoinSet<Update>,
    ready: &mut bool,
) -> Result<()> {
    match &mut update {
        Update::Auth(AuthState::Ready {
            account: actual, ..
        }) => {
            if actual != account {
                return Err(Error::Invalid(
                    "Authentication changed account identity".into(),
                ));
            }
            if !*ready {
                *ready = true;
                if let Some(items) = db
                    .get::<Vec<Conversation>>(&conversation_key(account))
                    .map_err(db_error)?
                {
                    let _ = send.send(Update::Conversations {
                        page: Page { items, next: None },
                        append: false,
                    });
                }
                let a = adapter.clone();
                jobs.spawn(async move {
                    match a.conversations(PageRequest::default()).await {
                        Ok(page) => Update::Conversations {
                            page,
                            append: false,
                        },
                        Err(error) => Update::Error(error),
                    }
                });
            }
        }
        Update::Conversations { page, append } => {
            let mut chats = if *append {
                db.get::<Vec<Conversation>>(&conversation_key(account))
                    .map_err(db_error)?
                    .unwrap_or_default()
            } else {
                vec![]
            };
            for chat in &page.items {
                account.require_chat(&chat.id)?;
                if let Some(old) = chats.iter_mut().find(|old| old.id == chat.id) {
                    *old = chat.clone();
                } else {
                    chats.push(chat.clone());
                }
            }
            db.put(&conversation_key(account), &chats)
                .await
                .map_err(db_error)?;
        }
        Update::History {
            chat,
            page,
            cached: false,
        } => {
            account.require_chat(chat)?;
            for message in &mut page.items {
                if &message.id.chat != chat {
                    return Err(Error::Invalid(
                        "History belongs to another conversation".into(),
                    ));
                }
                prepare_message(db, account, message)?;
                save_message(db, message).await?;
            }
        }
        Update::Sent {
            result: Ok(SendOutcome::Confirmed(message)),
            ..
        } => {
            prepare_message(db, account, message)?;
            save_message(db, message).await?;
        }
        Update::Error(error) => {
            tracing::error!(%error,adapter=account.adapter,"adapter operation failed")
        }
        _ => {}
    }
    if !matches!(update, Update::Auth(_)) {
        let _ = send.send(update);
    }
    Ok(())
}

/// Open the host store without initializing a GUI.
pub fn open_store(path: &Path) -> anyhow::Result<Arc<dyn Store>> {
    Ok(Arc::new(DbStore {
        db: Arc::new(Database::open(path)?),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn mock_adapter_sends_custom_events_and_persists_protobuf_with_account_isolation() {
        let root = tempfile::tempdir().unwrap();
        let mut backend = Backend::start(
            root.path().to_owned(),
            Box::new(|_| Arc::new(chat_adapter::mock::MockAdapter::new("one"))),
        );
        let updates = backend.updates.as_mut().unwrap();
        let chat = loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), updates.recv())
                .await
                .unwrap()
                .unwrap()
            {
                Update::Conversations { page, .. } => break page.items[0].id.clone(),
                Update::Error(error) => panic!("{error}"),
                _ => {}
            }
        };
        backend
            .commands
            .send(Command::History(chat.clone(), PageRequest::default()))
            .unwrap();
        let fetched = loop {
            match updates.recv().await.unwrap() {
                Update::History {
                    page,
                    cached: false,
                    ..
                } => {
                    assert!(page.items.iter().any(|message|message.content.iter().any(|part|matches!(part,Content::Extension(extension) if extension.kind=="example.poke"))));
                    assert!(page.next.is_none());
                    break page.items.len();
                }
                Update::Error(error) => panic!("{error}"),
                _ => {}
            }
        };
        backend
            .commands
            .send(Command::Send(SendRequest {
                chat: chat.clone(),
                client_id: "opaque-id".into(),
                content: vec![Content::Text("hello".into())],
                reply_to: None,
            }))
            .unwrap();
        loop {
            match updates.recv().await.unwrap() {
                Update::Sent { result: Ok(_), .. } => break,
                Update::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
        backend.commands.send(Command::Close).unwrap();
        while !*backend.stopped.borrow() {
            backend.stopped.changed().await.unwrap();
        }
        drop(backend);
        let db = Database::open(root.path()).unwrap();
        let messages = cached_messages(&db, &chat, 100).unwrap();
        assert_eq!(messages.len(), fetched + 1);
        assert!(
            cached_messages(
                &db,
                &AccountId {
                    adapter: "mock".into(),
                    id: "two".into()
                }
                .chat("room"),
                100
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            messages
                .iter()
                .any(|message| message.client_id.as_deref() == Some("opaque-id"))
        );
        db.close().await.unwrap();
    }
    #[tokio::test]
    async fn two_running_accounts_receive_independently_and_close_independently() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Backend::start(
            root.path().join("first"),
            Box::new(|_| Arc::new(chat_adapter::mock::MockAdapter::new("first"))),
        );
        let mut second = Backend::start(
            root.path().join("second"),
            Box::new(|_| Arc::new(chat_adapter::mock::MockAdapter::new("second"))),
        );
        async fn chat(backend: &mut Backend) -> ChatId {
            loop {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    backend.updates.as_mut().unwrap().recv(),
                )
                .await
                .unwrap()
                .unwrap()
                {
                    Update::Conversations { page, .. } => return page.items[0].id.clone(),
                    Update::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
        }
        let first_chat = chat(&mut first).await;
        let second_chat = chat(&mut second).await;
        assert_ne!(first_chat.account, second_chat.account);
        first.commands.send(Command::Close).unwrap();
        while !*first.stopped.borrow() {
            first.stopped.changed().await.unwrap();
        }
        assert!(!*second.stopped.borrow());
        second
            .commands
            .send(Command::Send(SendRequest {
                chat: second_chat,
                client_id: "after-switch".into(),
                content: vec![Content::Text("Still connected".into())],
                reply_to: None,
            }))
            .unwrap();
        loop {
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                second.updates.as_mut().unwrap().recv(),
            )
            .await
            .unwrap()
            .unwrap()
            {
                Update::Sent {
                    result: Ok(SendOutcome::Confirmed(message)),
                    ..
                } => {
                    assert_eq!(message.id.chat.account.id, "second");
                    break;
                }
                Update::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
        second.commands.send(Command::Close).unwrap();
        while !*second.stopped.borrow() {
            second.stopped.changed().await.unwrap();
        }
    }
    #[tokio::test]
    async fn arbitrary_session_records_roundtrip_through_the_hosts_protobuf_codec() {
        let root = tempfile::tempdir().unwrap();
        let store = open_store(root.path()).unwrap();
        let value = serde_json::json!({"key":"not-a-real-key","peer":9223372036854775807i64,"optional":null,"vector":[1,2,3]});
        store
            .put("telegram/one/session".into(), value.clone())
            .await
            .unwrap();
        assert_eq!(
            store.get("telegram/one/session".into()).await.unwrap(),
            Some(value)
        );
        assert!(
            store
                .get("telegram/two/session".into())
                .await
                .unwrap()
                .is_none()
        );
    }
}
