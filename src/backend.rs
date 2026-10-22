use crate::{
    model::{Chat, Cursor, conversation_key},
    storage::{Database, Session},
};
use anyhow::{Context as _, Result, bail};
use atri_qq::{
    Client, ClientConfig, Keystore, LoginEvent, ServiceClient, UserInfo,
    contacts::{Friend, Group, GroupAnnouncement, GroupMember, Operations},
    media::{Media, MediaInput, MediaReference},
    messages::{
        Conversation, ForwardReference, Message, MessageElement, Messaging, PushEvent, Reply,
        RoamDirection,
    },
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy)]
pub enum AttachmentKind {
    Image,
    File,
    Voice,
    Video,
}
#[derive(Clone)]
pub enum Command {
    SubmitCaptcha {
        ticket: String,
        random: String,
    },
    Preview {
        key: String,
        reference: MediaReference,
    },
    CardPreview {
        key: String,
        path: PathBuf,
        slot: usize,
        width: u32,
        height: u32,
    },
    Login {
        uin: u64,
        password: String,
    },
    Qr,
    Select(Conversation),
    History {
        conversation: Conversation,
        cursor: Option<Cursor>,
    },
    LocateReply {
        conversation: Conversation,
        reply: Reply,
    },
    Send {
        id: u64,
        nonce: u32,
        conversation: Conversation,
        elements: Vec<MessageElement>,
    },
    Attachment {
        id: u64,
        nonce: u32,
        conversation: Conversation,
        path: PathBuf,
        kind: AttachmentKind,
    },
    Asset {
        key: String,
        url: String,
    },
    ViewImage {
        key: String,
        reference: MediaReference,
    },
    ViewVideo {
        key: String,
        reference: MediaReference,
    },
    OpenMedia(MediaReference),
    GroupFile {
        group: u64,
        file: String,
    },
    Forward {
        key: String,
        reference: ForwardReference,
        is_group: bool,
    },
    Announcements(u64),
    Recall(Message),
    SaveChats(Vec<Chat>),
}
pub enum Update {
    LoginRequired,
    Challenge(atri_qq::LoginChallenge),
    Account(u64),
    Connecting,
    Qr {
        bytes: Vec<u8>,
        url: String,
    },
    Status(String),
    Online(UserInfo),
    Offline(String),
    Contacts {
        friends: Vec<Friend>,
        groups: Vec<Group>,
    },
    Restored(Box<crate::model::Model>),
    Members {
        group: u64,
        members: Vec<GroupMember>,
    },
    Announcements {
        group: u64,
        result: Result<Vec<GroupAnnouncement>, String>,
    },
    Message(Message),
    CachedMessages {
        key: String,
        messages: Vec<Message>,
    },
    History {
        key: String,
        result: Result<(Vec<Message>, Option<Cursor>), String>,
    },
    ReplyHistory {
        key: String,
        reply: Reply,
        result: Result<Vec<Message>, String>,
    },
    Sent {
        id: u64,
        result: Result<Message, String>,
    },
    Asset {
        key: String,
        path: PathBuf,
    },
    Image {
        key: String,
        result: Result<PathBuf, String>,
    },
    Video {
        key: String,
        result: Result<PathBuf, String>,
    },
    Forward {
        key: String,
        messages: Vec<Message>,
    },
    ForwardFailed {
        key: String,
        error: String,
    },
    Recalled {
        conversation: Conversation,
        sequence: u64,
    },
    Notification(PushEvent),
    Error(String),
}
pub struct Backend {
    pub commands: mpsc::Sender<Command>,
    pub updates: Option<mpsc::Receiver<Update>>,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Backend {
    pub fn start(root: PathBuf) -> Self {
        let (commands, receiver) = mpsc::channel(128);
        let (sender, updates) = mpsc::channel(512);
        let (stop, stopped) = oneshot::channel();
        let worker = thread::Builder::new()
            .name("ferrsphen".into())
            .spawn(move || {
                let executor = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(3)
                    .enable_all()
                    .build()
                    .expect("Tokio runtime");
                executor.block_on(async {
                    if let Err(error) = run(root, receiver, sender.clone(), stopped).await {
                        tracing::error!(error = ?error);
                        let _ = sender.try_send(Update::Error(error.to_string()));
                    }
                });
            })
            .expect("backend thread");
        Self {
            commands,
            updates: Some(updates),
            stop: Some(stop),
            worker: Some(worker),
        }
    }
    pub fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        drop(self.updates.take());
        drop(self.commands);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn fresh_device(old: &Keystore) -> Keystore {
    Keystore {
        guid: old.guid.clone(),
        device_name: old.device_name.clone(),
        android_id: old.android_id.clone(),
        qimei: old.qimei.clone(),
        ..Keystore::default()
    }
}
async fn emit(sender: &mpsc::Sender<Update>, update: Update) -> Result<()> {
    sender
        .send(update)
        .await
        .map_err(|_| anyhow::anyhow!("window closed"))
}
async fn run(
    root: PathBuf,
    mut commands: mpsc::Receiver<Command>,
    sender: mpsc::Sender<Update>,
    mut stopped: oneshot::Receiver<()>,
) -> Result<()> {
    let db = Database::open(&root)?;
    let mut session = db.session()?.unwrap_or_else(|| Session {
        keystore: Keystore::default(),
        machine_id: hex::encode(rand::random::<[u8; 16]>()),
        qua: atri_qq::Profile::linux().qua,
    });
    db.put("session/active", &session).await?;
    emit(&sender, Update::Account(session.keystore.uin)).await?;
    let mut password = None;
    let mut active_client = None;
    let work = async {
        loop {
            let config = ClientConfig {
                machine_id: session.machine_id.clone(),
                profile: atri_qq::Profile {
                    qua: session.qua.clone(),
                    ..atri_qq::Profile::linux()
                },
                uin: session.keystore.uin,
                password: password.take(),
                ..ClientConfig::default()
            };
            let quick = !session.keystore.a2.is_empty() || !session.keystore.a1.is_empty();
            emit(
                &sender,
                if quick {
                    Update::Connecting
                } else {
                    Update::LoginRequired
                },
            )
            .await?;
            let client = Client::new(config, session.keystore.clone())?;
            active_client = Some(client.clone());
            let transport: Arc<dyn ServiceClient> = client.clone();
            let operations = Arc::new(Operations::new(transport.clone()));
            let media = Arc::new(Media::new(transport.clone()));
            let messaging = Arc::new(Messaging::new(transport, media.clone()));
            let mut events = client.login_events();
            let mut packets = client.subscribe();
            let login_client = client.clone();
            let mut login = tokio::spawn(async move { login_client.login().await });
            let mut login_running = true;
            let mut online = false;
            let mut jobs = tokio::task::JoinSet::<Update>::new();
            let outcome=async {
                loop {
                    tokio::select! {
                        result=&mut login,if login_running=>{
                            login_running=false;
                            match result {Ok(Ok(_))=>{},Ok(Err(error))=>{tracing::warn!(%error,"login");emit(&sender,Update::LoginRequired).await?;emit(&sender,Update::Error(error.to_string())).await?;},Err(error)=>emit(&sender,Update::Error(error.to_string())).await?}
                        }
                        event=events.recv()=>match event {
                            Ok(LoginEvent::QrCode{url,image})=>{
                                tracing::info!("QR code ready");
                                if let Ok(code)=qrcode::QrCode::new(url.as_bytes()){println!("{}",code.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(true).build());}
                                emit(&sender,Update::Qr{bytes:image,url}).await?;
                            }
                            Ok(LoginEvent::QrState(state))=>emit(&sender,Update::Status(format!("{state:?}"))).await?,
                            Ok(LoginEvent::KeystoreChanged(keystore))=>{session.keystore=keystore;db.put("session/active",&session).await?;}
                            Ok(LoginEvent::Online{..})=>{
                                online=true;let state=client.snapshot().await;session.keystore=state.keystore;db.put("session/active",&session).await?;
                                emit(&sender,Update::Online(UserInfo::from(&session.keystore))).await?;
                                emit(&sender,Update::Restored(Box::new(db.restore(session.keystore.uin)?))).await?;
                                let cached_friends=db.get::<Vec<Friend>>(&format!("friends/{}",session.keystore.uin))?.unwrap_or_default();
                                let cached_groups=db.get::<Vec<Group>>(&format!("groups/{}",session.keystore.uin))?.unwrap_or_default();
                                emit(&sender,Update::Contacts{friends:cached_friends.clone(),groups:cached_groups.clone()}).await?;
                                let operations=operations.clone();
                                let updates=sender.clone();
                                jobs.spawn(async move {
                                    let (friends,groups)=tokio::join!(operations.fetch_friends(false),operations.fetch_groups(false));
                                    let friends=match friends {
                                        Ok(friends)=>friends,
                                        Err(error)=>{tracing::error!(%error,"fetch friends");let _=updates.send(Update::Error(error.to_string())).await;cached_friends}
                                    };
                                    let groups=match groups {
                                        Ok(groups)=>groups,
                                        Err(error)=>{tracing::error!(%error,"fetch groups");let _=updates.send(Update::Error(error.to_string())).await;cached_groups}
                                    };
                                    Update::Contacts{friends,groups}
                                });
                            }
                            Ok(LoginEvent::Offline{reason})=>{online=false;emit(&sender,Update::Offline(reason)).await?;}
                            Ok(LoginEvent::Challenge(challenge))=>emit(&sender,Update::Challenge(challenge)).await?,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(count))=>emit(&sender,Update::Error(format!("登录事件丢失 {count}"))).await?,
                            Err(tokio::sync::broadcast::error::RecvError::Closed)=>break,
                        },
                        packet=packets.recv(),if online=>match packet {
                            Ok(packet)=>for event in messaging.decode_push(packet).await {
                                match event {PushEvent::Message{message}=>{db.save_message(session.keystore.uin,&message).await?;emit(&sender,Update::Message(message)).await?;},event=>{if let PushEvent::Recall{conversation,sequence,..}=&event {db.save_recall(session.keystore.uin,conversation,*sequence).await?;}db.put(&format!("notification/{}/{:020}",session.keystore.uin,now_millis()),&event).await?;emit(&sender,Update::Notification(event)).await?;}}
                            },
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(count))=>emit(&sender,Update::Error(format!("消息事件丢失 {count}"))).await?,
                            Err(tokio::sync::broadcast::error::RecvError::Closed)=>break,
                        },
                        job=jobs.join_next(),if !jobs.is_empty()=>{
                            let update=job.expect("pending job").unwrap_or_else(|error|Update::Error(error.to_string()));
                            match &update {
                                Update::Message(message)|Update::Sent{result:Ok(message),..}=>db.save_message(session.keystore.uin,message).await?,
                                Update::ReplyHistory{result:Ok(messages),..}=>{for message in messages{db.save_message(session.keystore.uin,message).await?;}},
                                Update::History{result:Ok((messages,_)),..}=>{for message in messages{db.save_message(session.keystore.uin,message).await?;}},
                                Update::Contacts{friends,groups}=>{db.put(&format!("friends/{}",session.keystore.uin),friends).await?;db.put(&format!("groups/{}",session.keystore.uin),groups).await?;},
                                Update::Announcements{group,result:Ok(items)}=>db.put(&format!("announcements/{}/{group}",session.keystore.uin),items).await?,
                                Update::Announcements{result:Err(error),..}=>tracing::error!(%error,"fetch group announcements"),
                                Update::Recalled{conversation,sequence}=>db.save_recall(session.keystore.uin,conversation,*sequence).await?,
                                Update::Members{group,members}=>db.put(&format!("members/{}/{group}",session.keystore.uin),members).await?,
                                Update::Sent{id,result:Err(error)}=>tracing::error!(id,%error,"send message failed"),
                                Update::ReplyHistory{key,result:Err(error),..}=>tracing::error!(conversation=key,%error,"locate reply failed"),
                                Update::History{key,result:Err(error)}=>tracing::error!(conversation=key,%error,"load history failed"),
                                Update::ForwardFailed{key,error}=>tracing::error!(resource=key,%error,"load forward failed"),
                                Update::Image{result:Err(error),..}=>tracing::warn!(%error,"load image"),
                                Update::Error(error)=>tracing::error!(%error),_=>{}
                            }
                            emit(&sender,update).await?;
                        }
                        command=commands.recv(),if jobs.len()<32=>match command {
                            None=>break,
                            Some(Command::Login{uin,password:input})=>{session.keystore=fresh_device(&session.keystore);session.keystore.uin=uin;password=Some(input);return Ok::<bool,anyhow::Error>(true);}
                            Some(Command::Qr)=>{session.keystore=fresh_device(&session.keystore);return Ok(true);}
                            Some(Command::Asset{key,url})=>{
                                let path=root.join("cache");jobs.spawn(async move{match fetch_asset(&path,&key,&url).await{Ok(path)=>Update::Asset{key,path},Err(error)=>{tracing::warn!(%error,asset=key,"load asset failed");Update::Status(String::new())}}});
                            }
                            Some(Command::SubmitCaptcha{ticket,random})=>{let client=client.clone();jobs.spawn(async move{match client.submit_captcha(ticket,random).await{Ok(())=>Update::Status("正在验证".into()),Err(error)=>Update::Error(error.to_string())}});}
                            Some(Command::SaveChats(chats))=>{db.put(&format!("chats/{}",session.keystore.uin),&chats).await?;}
                            Some(command) if !online=>{let _=command;emit(&sender,Update::Error("账号离线".into())).await?;}
                            Some(Command::Select(conversation))=>{
                                let key=conversation_key(&conversation);let messages=db.messages(session.keystore.uin,&key,300)?;emit(&sender,Update::CachedMessages{key,messages}).await?;
                                if let Conversation::Group{uin}=conversation {
                                    if let Some(items)=db.get::<Vec<GroupAnnouncement>>(&format!("announcements/{}/{uin}",session.keystore.uin))? {
                                        emit(&sender,Update::Announcements{group:uin,result:Ok(items)}).await?;
                                    }
                                    let notices=operations.clone();
                                    jobs.spawn(async move{Update::Announcements{group:uin,result:notices.fetch_group_announcements(uin).await.map_err(|error|error.to_string())}});
                                    let operations=operations.clone();
                                    jobs.spawn(async move{match operations.fetch_members(uin,false).await{Ok(members)=>Update::Members{group:uin,members},Err(error)=>Update::Error(error.to_string())}});
                                }
                            }
                            Some(Command::Announcements(group))=>{let operations=operations.clone();jobs.spawn(async move{Update::Announcements{group,result:operations.fetch_group_announcements(group).await.map_err(|error|error.to_string())}});}
                            Some(Command::History{conversation,cursor})=>{let operations=operations.clone();let messaging=messaging.clone();jobs.spawn(async move{let key=conversation_key(&conversation);let result=load_history(&operations,&messaging,&conversation,cursor).await.map_err(|error|error.to_string());Update::History{key,result}});}
                            Some(Command::LocateReply{conversation,reply})=>{let messaging=messaging.clone();jobs.spawn(async move{let key=conversation_key(&conversation);let result=locate_reply(&messaging,&conversation,&reply).await.map_err(|error|format!("{error:#}"));Update::ReplyHistory{key,reply,result}});}
                            Some(Command::Send{id,nonce,conversation,elements})=>{let messaging=messaging.clone();jobs.spawn(async move{Update::Sent{id,result:messaging.send_with_nonce(conversation,elements,nonce).await.map_err(|error|error.to_string())}});}
                            Some(Command::Attachment{id,nonce,conversation,path,kind})=>{let messaging=messaging.clone();let media=media.clone();jobs.spawn(async move{Update::Sent{id,result:send_attachment(&messaging,&media,conversation,path,kind,nonce).await.map_err(|error|error.to_string())}});}
                            Some(Command::CardPreview{key,path,slot,width,height})=>{let cache=root.join("cache/cards");jobs.spawn(async move{let result=tokio::task::spawn_blocking(move||prepare_card(&cache,&key,&path,slot,width,height).map(|path|(key,path))).await;match result { Ok(Ok((key,path)))=>Update::Asset{key,path}, Ok(Err(error))=>{tracing::warn!(?error,"prepare stacked card failed");Update::Status(String::new())},Err(error)=>Update::Error(error.to_string()) }});}
                            Some(Command::Preview{key,reference})=>{let media=media.clone();let path=root.join("cache");jobs.spawn(async move{let result=async{let url=media.download_url(&reference).await?;fetch_asset(&path,&key,&url).await}.await;match result{Ok(path)=>Update::Asset{key,path},Err(error)=>{tracing::warn!(%error,asset=key,"load preview failed");Update::Status(String::new())}}});}
                            Some(Command::ViewImage{key,reference})=>{let media=media.clone();let root=root.join("cache");jobs.spawn(async move{let result=async {let url=media.download_url(&reference).await?;fetch_asset(&root,&key,&url).await}.await.map_err(|error:anyhow::Error|error.to_string());Update::Image{key,result}});}
                            Some(Command::ViewVideo{key,reference})=>{let media=media.clone();let root=root.join("cache/videos");jobs.spawn(async move{let result=download_video(&media,&reference,&root,&key).await.map_err(|error|format!("{error:#}"));if let Err(error)=&result{tracing::error!(%error,asset=key,"download video failed");}Update::Video{key,result}});}
                            Some(Command::OpenMedia(reference))=>{let media=media.clone();let path=root.join("downloads");jobs.spawn(async move{match download_open(&media,&reference,&path).await{Ok(())=>Update::Status(String::new()),Err(error)=>Update::Error(error.to_string())}});}
                            Some(Command::GroupFile{group,file})=>{let media=media.clone();jobs.spawn(async move{match media.group_file_download_url(group,&file).await {Ok(url)=>match open::that(url){Ok(_)=>Update::Status(String::new()),Err(error)=>Update::Error(error.to_string())},Err(error)=>Update::Error(error.to_string())}});}
                            Some(Command::Forward{key,reference,is_group})=>{let messaging=messaging.clone();jobs.spawn(async move{match messaging.download_forward(&reference.resource_id,is_group).await{Ok(messages) if !messages.is_empty()=>Update::Forward{key,messages},Ok(_)=>Update::ForwardFailed{key,error:"聊天记录为空".into()},Err(error)=>Update::ForwardFailed{key,error:error.to_string()}}});}
                            Some(Command::Recall(message))=>{if !crate::model::can_recall(&message,chrono::Utc::now().timestamp()){emit(&sender,Update::Error("已超过撤回时间".into())).await?;continue;}let messaging=messaging.clone();jobs.spawn(async move{match messaging.recall(&message).await{Ok(())=>Update::Recalled{conversation:message.conversation,sequence:message.sequence},Err(error)=>Update::Error(error.to_string())}});}
                        }
                    }
                }
                Ok(false)
            }.await;
            if login_running {
                login.abort();
                let _ = login.await;
            }
            jobs.abort_all();
            while jobs.join_next().await.is_some() {}
            client.close().await;
            if !outcome? {
                return Ok::<(), anyhow::Error>(());
            }
        }
    };
    let result = tokio::select! {result=work=>result,_=&mut stopped=>Ok(())};
    if let Some(client) = active_client {
        client.close().await;
    }
    db.close().await?;
    result
}
fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
async fn load_history(
    operations: &Operations,
    messaging: &Messaging,
    conversation: &Conversation,
    cursor: Option<Cursor>,
) -> Result<(Vec<Message>, Option<Cursor>)> {
    match conversation {
        Conversation::Group { uin } => {
            let end = match cursor {
                Some(Cursor::Group(end)) => end,
                None => {
                    operations
                        .fetch_group_extra(*uin)
                        .await?
                        .latest_message_sequence
                }
                _ => bail!("历史游标不匹配"),
            };
            if end == 0 {
                return Ok((Vec::new(), None));
            }
            let start = end.saturating_sub(29).max(1);
            let rows = messaging.history_group(*uin, start, end).await?;
            Ok((rows, (start > 1).then_some(Cursor::Group(start - 1))))
        }
        Conversation::Friend { uid, .. } => {
            let (time, random) = match cursor {
                Some(Cursor::Direct { time, random }) => (time, random),
                None => (
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)?
                        .as_secs()
                        .min(u64::from(u32::MAX)) as u32,
                    0,
                ),
                _ => bail!("历史游标不匹配"),
            };
            let page = messaging
                .roam(uid, time, random, 30, RoamDirection::Upwards)
                .await?;
            if page.peer_uid != *uid {
                bail!("历史会话不匹配");
            }
            let next = if page.is_complete {
                None
            } else {
                if (page.time, page.random) == (time, random) {
                    bail!("历史游标未前进");
                }
                Some(Cursor::Direct {
                    time: page.time,
                    random: page.random,
                })
            };
            Ok((page.messages, next))
        }
        _ => bail!("临时会话暂不支持历史记录"),
    }
}
async fn locate_reply(
    messaging: &Messaging,
    conversation: &Conversation,
    reply: &Reply,
) -> Result<Vec<Message>> {
    match conversation {
        Conversation::Group { uin } => {
            let sequence = reply
                .sequences
                .first()
                .copied()
                .filter(|sequence| *sequence != 0)
                .context("回复缺少消息序号")?;
            Ok(messaging
                .history_group(*uin, sequence.saturating_sub(29).max(1), sequence)
                .await?)
        }
        Conversation::Friend { uid, .. } => {
            let page = messaging
                .roam(
                    uid,
                    reply.time.saturating_add(1),
                    0,
                    30,
                    RoamDirection::Upwards,
                )
                .await?;
            if page.peer_uid != *uid {
                bail!("历史会话不匹配");
            }
            Ok(page.messages)
        }
        _ => bail!("临时会话暂不支持历史记录"),
    }
}
fn prepare_card(
    root: &Path,
    key: &str,
    source: &Path,
    slot: usize,
    width: u32,
    height: u32,
) -> Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    let path = root.join(format!("{:016x}.png", hash.finish()));
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(root)?;
    let temporary = root.join(format!(".{:016x}.part", rand::random::<u64>()));
    if let Err(error) = crate::card_preview::render(source, &temporary, slot, width, height) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    std::fs::rename(temporary, &path)?;
    Ok(path)
}
async fn fetch_asset(root: &Path, key: &str, url: &str) -> Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    let path = root.join(format!("{:016x}", hash.finish()));
    if path.is_file() {
        return Ok(path);
    }
    tokio::fs::create_dir_all(root).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
            bail!("图片过大");
        }
        bytes.extend_from_slice(&chunk);
    }
    let temporary = root.join(format!(".{:016x}.part", rand::random::<u64>()));
    tokio::fs::write(&temporary, bytes).await?;
    if let Err(error) = tokio::fs::rename(&temporary, &path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        if !path.is_file() {
            return Err(error.into());
        }
    }
    Ok(path)
}
async fn download_video(
    media: &Media,
    reference: &MediaReference,
    root: &Path,
    key: &str,
) -> Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    let path = root.join(format!("{:016x}.mp4", hash.finish()));
    if path.is_file() {
        return Ok(path);
    }
    tokio::fs::create_dir_all(root).await?;
    let temporary = root.join(format!(".{:016x}.part", rand::random::<u64>()));
    if let Err(error) = media.download(reference, &temporary).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error.into());
    }
    if let Err(error) = tokio::fs::rename(&temporary, &path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        if !path.is_file() {
            return Err(error.into());
        }
    }
    Ok(path)
}
async fn download_open(media: &Media, reference: &MediaReference, root: &Path) -> Result<()> {
    tokio::fs::create_dir_all(root).await?;
    let name = Path::new(&reference.name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment");
    let path = root.join(format!("{}-{name}", now_millis()));
    media.download(reference, &path).await?;
    let path = if reference.kind == atri_qq::media::MediaKind::Voice {
        let bytes = tokio::fs::read(&path).await?;
        if bytes.starts_with(b"#!SILK_V3") || bytes.starts_with(b"\x02#!SILK_V3") {
            let wave =
                tokio::task::spawn_blocking(move || crate::voice::silk_to_wave(&bytes)).await??;
            let wave_path = path.with_extension("wav");
            tokio::fs::write(&wave_path, wave).await?;
            wave_path
        } else {
            path
        }
    } else {
        path
    };
    open::that(path)?;
    Ok(())
}
async fn send_attachment(
    messaging: &Messaging,
    media: &Media,
    conversation: Conversation,
    path: PathBuf,
    kind: AttachmentKind,
    nonce: u32,
) -> Result<Message> {
    let target = conversation.media_conversation()?;
    let input = MediaInput::File { path: path.clone() };
    let element = match kind {
        AttachmentKind::Image => {
            MessageElement::Image(media.upload_image(input, target, "").await?)
        }
        AttachmentKind::Voice => {
            let data = tokio::fs::read(&path).await?;
            let input = if data.starts_with(b"#!SILK_V3") || data.starts_with(b"\x02#!SILK_V3") {
                input
            } else {
                let output = tokio::process::Command::new("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(&path)
                    .args(["-f", "s16le", "-ar", "24000", "-ac", "1", "-"])
                    .output()
                    .await
                    .context("无法转换音频：需要 ffmpeg")?;
                if !output.status.success() {
                    bail!("音频转换失败");
                }
                let encoded = tokio::task::spawn_blocking(move || {
                    atri_qq::voice::encode_silk(&output.stdout, 24000, 24000, true)
                })
                .await??;
                MediaInput::Data {
                    data: encoded,
                    name: "voice.silk".into(),
                }
            };
            MessageElement::Voice(media.upload_voice(input, target).await?)
        }
        AttachmentKind::File => MessageElement::File(media.upload_file(input, target, "/").await?),
        AttachmentKind::Video => {
            let output = tokio::process::Command::new("ffprobe")
                .args([
                    "-v",
                    "quiet",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=width,height:format=duration",
                    "-of",
                    "json",
                ])
                .arg(path)
                .output()
                .await
                .context("无法读取视频信息：需要 ffprobe")?;
            if !output.status.success() {
                bail!("无法读取视频信息");
            }
            let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            let stream = &value["streams"][0];
            let width = stream["width"].as_u64().context("视频宽度缺失")? as u32;
            let height = stream["height"].as_u64().context("视频高度缺失")? as u32;
            let duration = value["format"]["duration"]
                .as_str()
                .context("视频时长缺失")?
                .parse::<f64>()?
                .ceil() as u32;
            MessageElement::Video(
                media
                    .upload_video(input, None, target, width, height, duration)
                    .await?,
            )
        }
    };
    Ok(messaging
        .send_with_nonce(conversation, vec![element], nonce)
        .await?)
}
