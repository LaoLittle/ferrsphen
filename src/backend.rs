use crate::{
    model::{Chat, Cursor, conversation_key},
    storage::{Database, Session},
};
use anyhow::{Context as _, Result, bail};
use atri_qq::{
    Client, ClientConfig, Keystore, LoginEvent, ServiceClient, UserInfo,
    admin::Admin,
    contacts::{Friend, Group, GroupAnnouncement, GroupMember, Operations},
    media::{Media, MediaInput, MediaReference},
    messages::{
        Conversation, ForwardReference, Message, MessageElement, Messaging, PushEvent, Reply,
        RoamDirection,
    },
    mini_app::{MiniApp, MiniAppShareRequest},
    sdk_share::{
        AndroidShareSource, MusicShareContent, MusicShareReceipt, MusicShareTarget, SdkShare,
    },
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    File,
    Voice,
    Video,
}
#[derive(Clone)]
pub enum ComposePart {
    Element(Box<MessageElement>),
    Image(PathBuf),
}
#[derive(Clone)]
pub enum Command {
    OpenCaptcha {
        id: u64,
        request: crate::captcha::Request,
    },
    CancelCaptcha,
    SavePreferences(crate::settings::Preferences),
    Reaction {
        group: u64,
        sequence: u64,
        code: String,
        added: bool,
    },
    Favorites,
    Poke {
        conversation: Conversation,
        target: u64,
        uid: String,
    },
    SubmitCaptcha {
        ticket: String,
        random: String,
    },
    Preview {
        key: String,
        reference: MediaReference,
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
    ShareMusic {
        id: u64,
        conversation: Conversation,
        source: AndroidShareSource,
        content: MusicShareContent,
    },
    SendCard {
        id: u64,
        nonce: u32,
        conversation: Conversation,
        request: MiniAppShareRequest,
    },
    Compose {
        id: u64,
        nonce: u32,
        conversation: Conversation,
        parts: Vec<ComposePart>,
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
    ResolveUser {
        uid: String,
    },
    ViewImage {
        key: String,
        reference: MediaReference,
    },
    ViewVideo {
        key: String,
        reference: MediaReference,
    },
    Voice {
        key: String,
        reference: MediaReference,
    },
    VideoPreview {
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
    MusicShared {
        id: u64,
        result: Result<MusicShareReceipt, String>,
    },
    Captcha {
        id: u64,
        result: Result<Option<crate::captcha::Ticket>, String>,
    },
    AssetFailed {
        key: String,
    },
    Preferences(crate::settings::Preferences),
    Activity(crate::activity::ActivityRecord),
    ReactionDone {
        group: u64,
        sequence: u64,
        code: String,
        result: Result<crate::activity::ActivityRecord, String>,
    },
    Favorites {
        cached: bool,
        result: Result<Vec<crate::expressions::Favorite>, String>,
    },
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
    UserUin {
        uid: String,
        result: Result<u64, String>,
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
    Voice {
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
        Self::start_bound(root, None)
    }
    pub fn start_bound(root: PathBuf, expected_uin: Option<u64>) -> Self {
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
                    if let Err(error) =
                        run(root, receiver, sender.clone(), stopped, expected_uin).await
                    {
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
        ios_msf_imei: old.ios_msf_imei.clone(),
        ios_virtual_state: old.ios_virtual_state.clone(),
        device_name: old.device_name.clone(),
        android_id: old.android_id.clone(),
        qimei: old.qimei.clone(),
        o3_xwid_switch: old.o3_xwid_switch,
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
    mut expected_uin: Option<u64>,
) -> Result<()> {
    let db = Database::open(&root)?;
    let obsolete_cards = root.join("cache/cards");
    if obsolete_cards.is_dir()
        && let Err(error) = tokio::fs::remove_dir_all(&obsolete_cards).await
    {
        tracing::warn!(?error, "remove obsolete card images failed");
    }
    let migrated = db.migrate_legacy().await?;
    if migrated != 0 {
        tracing::info!(records = migrated, "migrated database records to protobuf");
    }
    emit(
        &sender,
        Update::Preferences(db.get(crate::settings::STORAGE_KEY)?.unwrap_or_default()),
    )
    .await?;
    let mut session = db.session()?.unwrap_or_else(|| Session {
        keystore: Keystore::default(),
        machine_id: hex::encode(rand::random::<[u8; 16]>()),
        qua: atri_qq::Profile::linux().qua,
    });
    db.save_session(&session).await?;
    emit(&sender, Update::Account(session.keystore.uin)).await?;
    let image_cache = Arc::new(crate::image_cache::ImageCache::new(&root.join("cache")));
    let mut password = None;
    let mut active_client = None;
    let work = async {
        loop {
            let config = ClientConfig {
                profile: atri_qq::Profile {
                    qua: session.qua.clone(),
                    ..atri_qq::Profile::linux()
                },
                platform: Some(atri_qq::PlatformConfig::Linux(atri_qq::LinuxConfig {
                    machine_id: session.machine_id.clone(),
                    ..Default::default()
                })),
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
            let sdk_share = Arc::new(SdkShare::new(transport.clone()));
            let mini_app = Arc::new(MiniApp::new(transport.clone()));
            let messaging = Arc::new(Messaging::new(transport, media.clone()));
            let mut events = client.login_events();
            let mut packets = client.subscribe();
            let login_client = client.clone();
            let mut login = tokio::spawn(async move { login_client.login().await });
            let mut login_running = true;
            let mut online = false;
            let mut jobs = tokio::task::JoinSet::<Update>::new();
            let mut captcha_job: Option<tokio::task::AbortHandle> = None;
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
                            Ok(LoginEvent::KeystoreChanged(keystore))=>{
                                if expected_uin.is_some_and(|expected| keystore.uin != 0 && keystore.uin != expected) { anyhow::bail!("This profile belongs to another QQ account"); }
                                session.keystore=keystore;db.save_session(&session).await?;}
                            Ok(LoginEvent::Online{..})=>{
                                let state=client.snapshot().await;
                                if expected_uin.is_some_and(|expected| state.keystore.uin != expected) { anyhow::bail!("This profile belongs to another QQ account"); }
                                expected_uin=Some(state.keystore.uin);
                                online=true;session.keystore=state.keystore;db.save_session(&session).await?;
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
                                match event {PushEvent::Message{message}=>{db.save_message(session.keystore.uin,&message).await?;emit(&sender,Update::Message(message)).await?;},event=>{if let PushEvent::Recall{conversation,sequence,..}=&event {tracing::info!(?event,"QQ recall received");db.save_recall(session.keystore.uin,conversation,*sequence).await?;}let record=db.save_notification(session.keystore.uin,&event).await?;if matches!(event,PushEvent::Poke{..}|PushEvent::Reaction{..}){emit(&sender,Update::Activity(record)).await?;}else{emit(&sender,Update::Notification(event)).await?;}}}
                            },
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(count))=>emit(&sender,Update::Error(format!("消息事件丢失 {count}"))).await?,
                            Err(tokio::sync::broadcast::error::RecvError::Closed)=>break,
                        },
                        job=jobs.join_next(),if !jobs.is_empty()=>{
                            let update=match job.expect("pending job") {
                                Ok(update)=>update,
                                Err(error) if error.is_cancelled()=>continue,
                                Err(error)=>Update::Error(error.to_string()),
                            };
                            match &update {
                                Update::Message(message)|Update::Sent{result:Ok(message),..}=>db.save_message(session.keystore.uin,message).await?,
                                Update::ReplyHistory{result:Ok(messages),..}=>{for message in messages{db.save_message(session.keystore.uin,message).await?;}},
                                Update::History{result:Ok((messages,_)),..}=>{for message in messages{db.save_message(session.keystore.uin,message).await?;}},
                                Update::Contacts{friends,groups}=>{db.put(&format!("friends/{}",session.keystore.uin),friends).await?;db.put(&format!("groups/{}",session.keystore.uin),groups).await?;},
                                Update::ReactionDone{result:Ok(record),..}=>db.save_activity(session.keystore.uin,record).await?,
                                Update::ReactionDone{result:Err(error),..}=>tracing::error!(%error,"set group reaction failed"),
                                Update::Favorites{cached:false,result:Ok(items)}=>db.put(&format!("favorites/{}",session.keystore.uin),items).await?,
                                Update::Favorites{result:Err(error),..}=>tracing::error!(%error,"fetch favorite expressions failed"),
                                Update::Announcements{group,result:Ok(items)}=>db.put(&format!("announcements/{}/{group}",session.keystore.uin),items).await?,
                                Update::Announcements{result:Err(error),..}=>tracing::error!(%error,"fetch group announcements"),
                                Update::UserUin{uid,result:Ok(uin)}=>db.save_user_uin(session.keystore.uin,uid,*uin).await?,
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
                            Some(Command::Login{uin,password:input})=>{
                                if expected_uin.is_some_and(|expected| expected != uin) { emit(&sender,Update::Error("This profile belongs to another QQ account".into())).await?;continue; }
                                session.keystore=fresh_device(&session.keystore);session.keystore.uin=uin;password=Some(input);return Ok::<bool,anyhow::Error>(true);}
                            Some(Command::Qr)=>{session.keystore=fresh_device(&session.keystore);return Ok(true);}
                            Some(Command::Favorites)=>{
                                if let Some(items)=db.get::<Vec<crate::expressions::Favorite>>(&format!("favorites/{}",session.keystore.uin))? {emit(&sender,Update::Favorites{cached:true,result:Ok(items)}).await?;}
                                if !online {emit(&sender,Update::Favorites{cached:false,result:Err("账号离线".into())}).await?;continue;}
                                let client=client.clone();
                                jobs.spawn(async move {Update::Favorites{cached:false,result:crate::expressions::fetch_favorites(client.as_ref()).await.map_err(|error|format!("{error:#}"))}});
                            }
                            Some(Command::Poke{conversation,target,uid})=>{
                                if !online {emit(&sender,Update::Error("账号离线".into())).await?;continue;}
                                let operations=operations.clone();
                                jobs.spawn(async move {
                                    let result=async {
                                        let target=if target!=0 {target} else {operations.resolve_uin(&uid).await?};
                                        anyhow::ensure!(target!=0,"无法识别用户");
                                        let admin=Admin::from_operations(operations.clone());
                                        match conversation {
                                            Conversation::Group{uin}=>admin.send_group_nudge(uin,target).await?,
                                            Conversation::Friend{uin,uid}=>{let peer=if uin!=0{uin}else{operations.resolve_uin(&uid).await?};admin.send_friend_nudge(peer,Some(target)).await?;},
                                            _=>bail!("临时会话暂不支持戳一戳"),
                                        }
                                        Ok::<_,anyhow::Error>(())
                                    }.await;
                                    match result {Ok(())=>Update::Status(String::new()),Err(error)=>{tracing::error!(?error,"poke failed");Update::Error(format!("{error:#}"))}}
                                });
                            }
                            Some(Command::Reaction{group,sequence,code,added})=>{
                                if !online {emit(&sender,Update::ReactionDone{group,sequence,code,result:Err("账号离线".into())}).await?;continue;}
                                let operations=operations.clone();
                                let client=client.clone();
                                jobs.spawn(async move {
                                    let result=async {
                                        anyhow::ensure!(group!=0&&sequence!=0,"消息尚未确认");
                                        let operator_uid=client.snapshot().await.keystore.uid;
                                        anyhow::ensure!(!operator_uid.is_empty(),"账号未登录");
                                        Admin::from_operations(operations).set_group_reaction(group,sequence,&code,added).await?;
                                        Ok(crate::activity::ActivityRecord::new(crate::activity::ActivityEvent::ReactionConfirmed{group,sequence,code:code.clone(),added,operator_uid}))
                                    }.await.map_err(|error:anyhow::Error|format!("{error:#}"));
                                    Update::ReactionDone{group,sequence,code,result}
                                });
                            }
                            Some(Command::Asset{key,url})=>{
                                let path=root.join("cache");let cache=image_cache.clone();jobs.spawn(async move{match load_asset(&cache,&path,&key,&url).await{Ok(path)=>Update::Asset{key,path},Err(error)=>{tracing::warn!(%error,asset=key,"load asset failed");Update::AssetFailed{key}}}});
                            }
                            Some(Command::OpenCaptcha{id,request})=>{
                                if let Some(job)=captcha_job.take(){job.abort();}
                                captcha_job=Some(jobs.spawn(async move {
                                    let result=crate::captcha::verify(request).await.map_err(|error|format!("{error:#}"));
                                    if let Err(error)=&result {tracing::warn!(%error,"open captcha failed");}
                                    Update::Captcha{id,result}
                                }));
                            }
                            Some(Command::CancelCaptcha)=>{if let Some(job)=captcha_job.take(){job.abort();}}
                            Some(Command::SubmitCaptcha{ticket,random})=>{let client=client.clone();jobs.spawn(async move{match client.submit_captcha(ticket,random).await{Ok(())=>Update::Status("正在验证".into()),Err(error)=>Update::Error(error.to_string())}});}
                            Some(Command::SavePreferences(preferences))=>{db.put(crate::settings::STORAGE_KEY,&preferences).await?;}
                            Some(Command::SaveChats(chats))=>{db.put(&format!("chats/{}",session.keystore.uin),&chats).await?;}
                            Some(Command::ResolveUser{uid})=>{
                                if let Some(uin)=db.user_uin(session.keystore.uin,&uid)? {
                                    emit(&sender,Update::UserUin{uid,result:Ok(uin)}).await?;
                                } else if !online {
                                    emit(&sender,Update::UserUin{uid,result:Err("账号离线".into())}).await?;
                                } else {
                                    let operations=operations.clone();
                                    jobs.spawn(async move{let result=operations.resolve_uin(&uid).await.and_then(|uin|if uin!=0{Ok(uin)}else{Err(atri_qq::Error::Malformed("UID resolved to zero UIN".into()))}).map_err(|error|format!("{error:#}"));Update::UserUin{uid,result}});
                                }
                            }
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
                            Some(Command::ShareMusic{id,conversation,source,content})=>{
                                let api=sdk_share.clone();let operations=operations.clone();
                                jobs.spawn(async move {
                                    let result=share_music(&api,&operations,conversation,source,content).await;
                                    if let Err(error)=&result {tracing::error!(?error,"music share failed");}
                                    Update::MusicShared{id,result:result.map_err(|error|format!("{error:#}"))}
                                });
                            }
                            Some(Command::SendCard{id,nonce,conversation,request})=>{let messaging=messaging.clone();let mini_app=mini_app.clone();jobs.spawn(async move{Update::Sent{id,result:send_mini_app_card(&messaging,&mini_app,conversation,request,nonce).await.map_err(|error|format!("{error:#}"))}});}
                            Some(Command::Compose{id,nonce,conversation,parts})=>{let messaging=messaging.clone();let media=media.clone();let cache=image_cache.clone();jobs.spawn(async move{for part in &parts {if let ComposePart::Image(path)=part && let Err(error)=cache.remember(path).await {tracing::debug!(%error,"cache outgoing image failed");}}Update::Sent{id,result:send_composed(&messaging,&media,conversation,parts,nonce).await.map_err(|error|format!("{error:#}"))}});}
                            Some(Command::Attachment{id,nonce,conversation,path,kind})=>{let messaging=messaging.clone();let media=media.clone();let cache=image_cache.clone();jobs.spawn(async move{if kind==AttachmentKind::Image && let Err(error)=cache.remember(&path).await {tracing::debug!(%error,"cache outgoing image failed");}Update::Sent{id,result:send_attachment(&messaging,&media,conversation,path,kind,nonce).await.map_err(|error|error.to_string())}});}
                            Some(Command::Preview{key,reference})=>{let media=media.clone();let cache=image_cache.clone();let path=root.join("cache");jobs.spawn(async move{let result=load_image(&media,&cache,&reference,&path,&key).await;match result{Ok(path)=>Update::Asset{key,path},Err(error)=>{tracing::warn!(%error,asset=key,"load preview failed");Update::AssetFailed{key}}}});}
                            Some(Command::ViewImage{key,reference})=>{let media=media.clone();let cache=image_cache.clone();let root=root.join("cache");jobs.spawn(async move{let result=load_image(&media,&cache,&reference,&root,&key).await.map_err(|error:anyhow::Error|error.to_string());Update::Image{key,result}});}
                            Some(Command::ViewVideo{key,reference})=>{let media=media.clone();let root=root.join("cache/videos");jobs.spawn(async move{let result=download_video(&media,&reference,&root,&key).await.map_err(|error|format!("{error:#}"));if let Err(error)=&result{tracing::error!(%error,asset=key,"download video failed");}Update::Video{key,result}});}
                            Some(Command::Voice{key,reference})=>{let media=media.clone();let root=root.join("cache/voices");jobs.spawn(async move{let result=download_voice(&media,&reference,&root,&key).await.map_err(|error|format!("{error:#}"));if let Err(error)=&result{tracing::error!(%error,asset=key,"prepare voice failed");}Update::Voice{key,result}});}
                            Some(Command::VideoPreview{key,reference})=>{let media=media.clone();let root=root.join("cache");jobs.spawn(async move{match video_preview(&media,&reference,&root,&key).await{Ok(path)=>Update::Asset{key,path},Err(error)=>{tracing::warn!(?error,asset=key,"load video preview failed");Update::Status(String::new())}}});}
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
            match group_history(messaging, *uin, start, end).await? {
                Some(rows) => Ok((rows, (start > 1).then_some(Cursor::Group(start - 1)))),
                None => Ok((Vec::new(), None)),
            }
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
async fn group_history(
    messaging: &Messaging,
    group: u64,
    start: u64,
    end: u64,
) -> Result<Option<Vec<Message>>> {
    match messaging.history_group(group, start, end).await {
        Ok(messages) => Ok(Some(messages)),
        Err(atri_qq::Error::Server { code: 4, .. }) => {
            tracing::info!(group, start, end, "group history exhausted");
            Ok(None)
        }
        Err(error) => Err(error.into()),
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
            Ok(group_history(
                messaging,
                *uin,
                sequence.saturating_sub(29).max(1),
                sequence,
            )
            .await?
            .unwrap_or_default())
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
async fn load_asset(
    cache: &crate::image_cache::ImageCache,
    root: &Path,
    key: &str,
    url: &str,
) -> Result<PathBuf> {
    let emoji = key.starts_with("face:");
    if emoji && let Some(path) = crate::emoji_assets::cached(root, key).await? {
        return Ok(path);
    }
    if emoji
        && let Some(extension) = ["svg", "json"]
            .into_iter()
            .find(|extension| url.ends_with(&format!(".{extension}")))
    {
        let original = fetch_asset(root, key, url).await?;
        return crate::emoji_assets::remember_vector(root, key, &original, extension).await;
    }
    let original = fetch_image(root, key, url).await?;
    let original = cache.remember(&original).await?;
    if emoji {
        let prepared = crate::emoji_assets::prepare(&original, root).await?;
        crate::emoji_assets::remember_key(root, key, &prepared).await
    } else {
        Ok(original)
    }
}
async fn load_image(
    media: &Media,
    cache: &crate::image_cache::ImageCache,
    reference: &MediaReference,
    root: &Path,
    key: &str,
) -> Result<PathBuf> {
    let _guard = cache.lock(reference, key).await;
    if let Some(path) = cache.find(reference).await? {
        return Ok(path);
    }
    // A per-message cache entry can also predate the content index.
    let path = if let Some(path) = cached_image(root, key) {
        path
    } else {
        let url = media.download_url(reference).await?;
        fetch_image(root, key, &url).await?
    };
    cache.remember(&path).await
}
fn cached_image(root: &Path, key: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    let name = format!("{:016x}", hash.finish());
    [
        "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "tiff", "ico", "qoi",
    ]
    .into_iter()
    .map(|extension| root.join("images").join(format!("{name}.{extension}")))
    .find(|path| path.is_file())
}
async fn fetch_image(root: &Path, key: &str, url: &str) -> Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    let name = format!("{:016x}", hash.finish());
    let directory = root.join("images");
    let cached = || {
        [
            "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "tiff", "ico", "qoi",
        ]
        .into_iter()
        .map(|extension| directory.join(format!("{name}.{extension}")))
        .find(|path| path.is_file())
    };
    if let Some(path) = cached() {
        return Ok(path);
    }
    let legacy = fetch_asset(root, key, url).await?;
    let bytes = match tokio::fs::read(&legacy).await {
        Ok(bytes) => bytes,
        Err(error) => {
            if let Some(path) = cached() {
                return Ok(path);
            }
            return Err(error.into());
        }
    };
    let format = image::guess_format(&bytes).context("图片格式无法识别")?;
    let extension = format
        .extensions_str()
        .first()
        .copied()
        .context("图片格式没有文件后缀")?;
    tokio::fs::create_dir_all(&directory).await?;
    let path = directory.join(format!("{name}.{extension}"));
    if let Err(error) = tokio::fs::rename(&legacy, &path).await {
        if !path.is_file() {
            return Err(error.into());
        }
        let _ = tokio::fs::remove_file(&legacy).await;
    }
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
    tracing::debug!(target: "atri_qq_protocol::packet_trace", direction = "send", transport = "http", method = "GET", url, "QQ asset request");
    let response = client.get(url).send().await?;
    tracing::debug!(target: "atri_qq_protocol::packet_trace", direction = "receive", transport = "http", url = %response.url(), status = response.status().as_u16(), headers = ?response.headers(), "QQ asset response");
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        tracing::debug!(target: "atri_qq_protocol::packet_trace", direction = "receive", transport = "http", url, offset = bytes.len(), length = chunk.len(), body_hex = %hex::encode(&chunk), "QQ asset body");
        if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
            bail!("图片过大");
        }
        bytes.extend_from_slice(&chunk);
    }
    response.error_for_status_ref()?;
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
fn media_cache_path(root: &Path, key: &str, extension: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    root.join(format!("{:016x}.{extension}", hash.finish()))
}
async fn download_video(
    media: &Media,
    reference: &MediaReference,
    root: &Path,
    key: &str,
) -> Result<PathBuf> {
    let path = media_cache_path(root, key, "mp4");
    if path.is_file() {
        return Ok(path);
    }
    let url = media
        .download_url(reference)
        .await
        .context("获取视频下载地址失败")?;
    let mut download = reference.clone();
    download.message_info.clear();
    download.download_url = Some(url);
    download_cached(media, &download, root, key, "mp4").await
}
async fn download_cached(
    media: &Media,
    reference: &MediaReference,
    root: &Path,
    key: &str,
    extension: &str,
) -> Result<PathBuf> {
    let path = media_cache_path(root, key, extension);
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
async fn download_voice(
    media: &Media,
    reference: &MediaReference,
    root: &Path,
    key: &str,
) -> Result<PathBuf> {
    let path = download_cached(media, reference, root, key, "audio")
        .await
        .with_context(|| format!("下载语音失败：{reference:?}"))?;
    let wave_path = path.with_extension("decoded-v2.wav");
    if wave_path.is_file() {
        return Ok(wave_path);
    }
    let bytes = tokio::fs::read(&path).await?;
    tracing::debug!(asset = key, ?reference, length = bytes.len(), header = %hex::encode(&bytes[..bytes.len().min(32)]), "voice downloaded");
    if bytes.starts_with(b"#!AMR") {
        let temporary = root.join(format!(".{:016x}.wav.part", rand::random::<u64>()));
        let output = tokio::process::Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y", "-i"])
            .arg(&path)
            .args([
                "-vn",
                "-ac",
                "1",
                "-ar",
                "24000",
                "-c:a",
                "pcm_s16le",
                "-f",
                "wav",
            ])
            .arg(&temporary)
            .output()
            .await
            .context("转换 AMR 语音需要 ffmpeg")?;
        if !output.status.success() {
            let _ = tokio::fs::remove_file(&temporary).await;
            bail!("AMR 转换失败：{}", String::from_utf8_lossy(&output.stderr));
        }
        tokio::fs::rename(&temporary, &wave_path).await?;
        return Ok(wave_path);
    }
    if bytes.starts_with(b"#!SILK_V3") || bytes.starts_with(b"\x02#!SILK_V3") {
        let wave = tokio::task::spawn_blocking(move || crate::voice::silk_to_wave(&bytes))
            .await?
            .with_context(|| format!("解码 SILK 语音失败：{}", path.display()))?;
        let temporary = root.join(format!(".{:016x}.wav.part", rand::random::<u64>()));
        tokio::fs::write(&temporary, wave).await?;
        if let Err(error) = tokio::fs::rename(&temporary, &wave_path).await {
            let _ = tokio::fs::remove_file(&temporary).await;
            if !wave_path.is_file() {
                return Err(error.into());
            }
        }
        return Ok(wave_path);
    }
    Ok(path)
}
async fn video_preview(
    media: &Media,
    reference: &MediaReference,
    root: &Path,
    key: &str,
) -> Result<PathBuf> {
    // The video service signs the thumbnail's own index.
    match media.video_thumbnail_url(reference).await {
        Ok(url) => match fetch_asset(root, key, &url).await {
            Ok(path) => return Ok(path),
            Err(error) => tracing::debug!(?error, "video thumbnail download failed"),
        },
        Err(error) => tracing::debug!(?error, "video thumbnail unavailable"),
    }
    #[cfg(feature = "desktop")]
    {
        let video = download_video(
            media,
            reference,
            &root.join("videos"),
            key.trim_start_matches("preview:"),
        )
        .await?;
        let output = video.with_extension("png");
        if output.is_file() {
            return Ok(output);
        }
        tokio::task::spawn_blocking(move || {
            crate::video::extract_preview(&video, &output)?;
            Ok(output)
        })
        .await?
    }
    #[cfg(not(feature = "desktop"))]
    {
        bail!("Video thumbnail unavailable")
    }
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
async fn share_music(
    api: &SdkShare,
    operations: &Operations,
    conversation: Conversation,
    source: AndroidShareSource,
    content: MusicShareContent,
) -> Result<MusicShareReceipt> {
    let target = match conversation {
        Conversation::Temporary { .. } => {
            bail!("Music sharing is unavailable in temporary conversations")
        }
        Conversation::Group { uin } => MusicShareTarget::Group { uin },
        Conversation::Friend { uin, uid } => MusicShareTarget::Friend {
            uin: if uin != 0 {
                uin
            } else {
                operations.resolve_uin(&uid).await?
            },
        },
    };
    let receipt = api
        .share_music(atri_qq::sdk_share::ShareMusicRequest {
            target,
            source,
            content,
        })
        .await?;
    tracing::info!(?receipt, "music share receipt");
    Ok(receipt)
}

async fn send_mini_app_card(
    messaging: &Messaging,
    mini_app: &MiniApp,
    conversation: Conversation,
    request: MiniAppShareRequest,
    nonce: u32,
) -> Result<Message> {
    anyhow::ensure!(
        !request.app_id.trim().is_empty(),
        "Mini-app share requires app_id"
    );
    let card = mini_app
        .adapt_share_info(request)
        .await
        .context("Adapt mini-app share info")?;
    messaging
        .send_with_nonce(
            conversation,
            vec![MessageElement::LightApp(card.light_app_json)],
            nonce,
        )
        .await
        .context("Send mini-app card")
}

async fn send_composed(
    messaging: &Messaging,
    media: &Media,
    conversation: Conversation,
    parts: Vec<ComposePart>,
    nonce: u32,
) -> Result<Message> {
    anyhow::ensure!(!parts.is_empty(), "消息为空");
    anyhow::ensure!(
        !parts.iter().any(|part| matches!(
            part,
            ComposePart::Element(element) if matches!(element.as_ref(), MessageElement::File(_) | MessageElement::Voice(_))
        )),
        "文件和语音需要单独发送"
    );
    let target = conversation.media_conversation()?;
    let mut elements = Vec::new();
    for part in parts {
        elements.push(match part {
            ComposePart::Element(element) => *element,
            ComposePart::Image(path) => MessageElement::Image(
                media
                    .upload_image(MediaInput::File { path }, target.clone(), "")
                    .await?,
            ),
        });
    }
    Ok(messaging
        .send_with_nonce(conversation, elements, nonce)
        .await?)
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
            MessageElement::Video(media.upload_video(input, None, target, 0, 0, 0).await?)
        }
    };
    Ok(messaging
        .send_with_nonce(conversation, vec![element], nonce)
        .await?)
}

#[cfg(test)]
mod asset_tests {
    use super::fetch_asset;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Default)]
    struct VideoClient {
        calls: std::sync::Mutex<Vec<(String, atri_qq::proto::media::MediaDownloadRequest)>>,
    }
    #[async_trait::async_trait]
    impl atri_qq::ServiceClient for VideoClient {
        async fn request(
            &self,
            command: &str,
            body: bytes::Bytes,
            _: atri_qq::RequestType,
            _: atri_qq::Encryption,
            _: std::time::Duration,
        ) -> atri_qq::Result<bytes::Bytes> {
            let envelope: atri_qq::proto::media::MediaOidbEnvelope =
                atri_qq::proto::decode(body).unwrap();
            let request = atri_qq::proto::decode(envelope.body.unwrap()).unwrap();
            self.calls.lock().unwrap().push((command.into(), request));
            Err(atri_qq::Error::Server {
                code: 1234,
                message: "download_url reached".into(),
            })
        }
        async fn snapshot(&self) -> atri_qq::SessionState {
            panic!("video download does not require message identities")
        }
        async fn replace_state(&self, _: atri_qq::SessionState) {
            unreachable!()
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<atri_qq::IncomingPacket> {
            tokio::sync::broadcast::channel(1).1
        }
    }

    #[tokio::test]
    async fn composed_messages_reject_standalone_media_before_uploading_images() {
        use super::*;
        use atri_qq::media::{MediaConversation, MediaKind};
        let client = Client::new(ClientConfig::default(), Keystore::default()).unwrap();
        let media = Arc::new(Media::new(client.clone()));
        let messaging = Messaging::new(client, media.clone());
        for kind in [MediaKind::File, MediaKind::Voice] {
            let reference = MediaReference::new(kind, MediaConversation::Group { uin: 42 });
            let element = if kind == MediaKind::File {
                MessageElement::File(reference)
            } else {
                MessageElement::Voice(reference)
            };
            let result = send_composed(
                &messaging,
                &media,
                Conversation::Group { uin: 42 },
                vec![
                    ComposePart::Image(PathBuf::from("missing.png")),
                    ComposePart::Element(Box::new(element)),
                ],
                1,
            )
            .await;
            assert!(result.unwrap_err().to_string().contains("单独发送"));
        }
    }

    #[tokio::test]
    async fn video_download_uses_updated_nt_request_without_legacy_identity() {
        use super::*;
        use atri_qq::{
            media::{MediaConversation, MediaKind},
            proto::media as pb,
        };
        use prost::Message as _;
        for (conversation, command) in [
            (
                MediaConversation::Friend { uid: "peer".into() },
                "OidbSvcTrpcTcp.0x11e9_200",
            ),
            (
                MediaConversation::Group { uin: 42 },
                "OidbSvcTrpcTcp.0x11ea_200",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let client = Arc::new(VideoClient::default());
            let media = Media::new(client.clone());
            let indices =
                [("nt-main", 0, 1234), ("nt-cover", 100, 64)].map(|(uuid, subtype, size)| {
                    pb::MediaIndex {
                        uuid: Some(uuid.as_bytes().to_vec().into()),
                        subtype: Some(subtype),
                        file: Some(pb::MediaFileInfo {
                            size: Some(size),
                            ..Default::default()
                        }),
                    }
                });
            let info = pb::MediaMessageInfo {
                bodies: indices
                    .iter()
                    .map(|index| {
                        pb::MediaMessageInfoBody {
                            index: Some(index.encode_to_vec().into()),
                        }
                        .encode_to_vec()
                        .into()
                    })
                    .collect(),
            }
            .encode_to_vec();
            let reference =
                MediaReference::from_message_info(info, MediaKind::Video, conversation, None)
                    .unwrap();
            assert!(reference.compatibility.is_none());
            let error = download_video(&media, &reference, root.path(), "video:test")
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains("download_url reached"));
            let calls = client.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, command);
            let request = &calls[0].1;
            assert_eq!(
                request
                    .head
                    .as_ref()
                    .unwrap()
                    .scene
                    .as_ref()
                    .unwrap()
                    .request_type,
                Some(2)
            );
            assert_eq!(
                request.request.as_ref().unwrap().index.as_deref(),
                Some(indices[0].encode_to_vec().as_slice())
            );
        }
    }

    #[tokio::test]
    async fn avatar_cache_is_bound_to_uin_and_reused_without_network() {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let received = stream.read(&mut request).await.unwrap();
            assert!(received > 0);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\navatar",
                )
                .await
                .unwrap();
        });
        let path = fetch_asset(
            root.path(),
            "avatar:77",
            &format!("http://{address}/avatar"),
        )
        .await
        .unwrap();
        server.await.unwrap();
        let cached = fetch_asset(root.path(), "avatar:77", "http://127.0.0.1:1/unavailable")
            .await
            .unwrap();
        assert_eq!(path, cached);
        assert_eq!(std::fs::read(cached).unwrap(), b"avatar");
        assert!(
            fetch_asset(root.path(), "avatar:78", "http://127.0.0.1:1/unavailable")
                .await
                .is_err()
        );
    }
}

#[cfg(test)]
mod session_tests {
    use super::{Keystore, fresh_device};

    #[test]
    fn restarting_login_keeps_server_signing_policy_and_clears_account_credentials() {
        for selector in [None, Some(0), Some(1), Some(2)] {
            let stored = Keystore {
                uin: 12345,
                uid: "old-account".into(),
                guid: vec![7; 16],
                device_name: "device".into(),
                ios_msf_imei: "A0000000-0000-4000-8000-000000000001".into(),
                android_id: "id".into(),
                qimei: "qimei".into(),
                o3_xwid_switch: selector,
                a1: vec![1],
                a2: vec![2],
                d2: vec![3],
                d2_key: vec![4],
                qr_sig: vec![5],
                key_exchange_session: Some(
                    atri_qq::NtKeyExchangeSession::new(
                        atri_qq::ClientPlatform::IPhone,
                        bytes::Bytes::from_static(&[7; 16]),
                        bytes::Bytes::from_static(b"synthetic-ticket"),
                        600,
                    )
                    .unwrap(),
                ),
                ..Default::default()
            };
            let fresh = fresh_device(&stored);
            assert_eq!(fresh.o3_xwid_switch, selector);
            assert_eq!(fresh.guid, stored.guid);
            assert_eq!(fresh.device_name, stored.device_name);
            assert_eq!(fresh.ios_msf_imei, stored.ios_msf_imei);
            assert!(fresh.key_exchange_session.is_none());
            assert_eq!(fresh.android_id, stored.android_id);
            assert_eq!(fresh.qimei, stored.qimei);
            assert_eq!(fresh.uin, 0);
            assert!(fresh.uid.is_empty());
            assert_eq!(fresh.d2_key, vec![0; 16]);
            assert!(
                fresh.a1.is_empty()
                    && fresh.a2.is_empty()
                    && fresh.d2.is_empty()
                    && fresh.qr_sig.is_empty()
            );
        }
    }
}

#[cfg(test)]
mod history_status_tests {
    use super::*;
    struct HistoryClient(i32);
    #[async_trait::async_trait]
    impl ServiceClient for HistoryClient {
        async fn request(
            &self,
            command: &str,
            _: bytes::Bytes,
            _: atri_qq::RequestType,
            _: atri_qq::Encryption,
            _: Duration,
        ) -> atri_qq::Result<bytes::Bytes> {
            assert_eq!(
                command,
                "trpc.msg.register_proxy.RegisterProxy.SsoGetGroupMsg"
            );
            use atri_qq::proto::messages::{GroupHistoryBody, GroupHistoryResponse};
            use prost::Message as _;
            Ok(GroupHistoryResponse {
                code: 0,
                error: None,
                body: Some(GroupHistoryBody {
                    code: i64::from(self.0),
                    group_uin: Some(42),
                    ..Default::default()
                }),
            }
            .encode_to_vec()
            .into())
        }
        async fn snapshot(&self) -> atri_qq::SessionState {
            unreachable!()
        }
        async fn replace_state(&self, _: atri_qq::SessionState) {
            unreachable!()
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<atri_qq::IncomingPacket> {
            tokio::sync::broadcast::channel(1).1
        }
    }
    #[tokio::test]
    async fn group_history_code_four_exhausts_cursor_and_other_errors_still_fail() {
        for code in [4, 5] {
            let client: Arc<dyn ServiceClient> = Arc::new(HistoryClient(code));
            let operations = Operations::new(client.clone());
            let messaging = Messaging::new(client.clone(), Arc::new(Media::new(client)));
            let result = load_history(
                &operations,
                &messaging,
                &Conversation::Group { uin: 42 },
                Some(Cursor::Group(100)),
            )
            .await;
            if code == 4 {
                let (messages, cursor) = result.unwrap();
                assert!(messages.is_empty());
                assert!(cursor.is_none());
            } else {
                assert!(matches!(
                    result.unwrap_err().downcast_ref::<atri_qq::Error>(),
                    Some(atri_qq::Error::Server { code: 5, .. })
                ));
            }
        }
    }
}

#[cfg(test)]
mod image_cache_tests {
    use super::fetch_image;
    #[tokio::test]
    async fn emoji_vectors_load_from_disk_after_restart_without_network_or_originals() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let animation_bytes =
            br#"{"v":"5.7.4","w":100,"h":100,"fr":30,"ip":0,"op":30,"layers":[]}"#.to_vec();
        let poster_bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><circle cx="50" cy="50" r="40" fill="red"/></svg>"#.to_vec();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (animation_body, poster_body) = (animation_bytes.clone(), poster_bytes.clone());
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }
                let body = if request.starts_with(b"GET /lottie.json ") {
                    &animation_body
                } else {
                    &poster_body
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
            }
        });
        let root = tempfile::tempdir().unwrap();
        let cache = crate::image_cache::ImageCache::new(root.path());
        let face = crate::expressions::Face::new(32);
        let animation = super::load_asset(
            &cache,
            root.path(),
            &face.asset_key(),
            &format!("http://{address}/lottie.json"),
        )
        .await
        .unwrap();
        let poster = super::load_asset(
            &cache,
            root.path(),
            &face.poster_asset_key(),
            &format!("http://{address}/emoji.svg"),
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(animation.parent().unwrap(), root.path().join("emojis"));
        assert_eq!(animation.extension().unwrap(), "json");
        assert_eq!(poster.extension().unwrap(), "svg");
        for entry in std::fs::read_dir(root.path()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                std::fs::remove_file(entry.path()).unwrap();
            }
        }
        drop(cache);
        let restarted = crate::image_cache::ImageCache::new(root.path());
        for (key, expected, bytes) in [
            (face.asset_key(), animation, animation_bytes),
            (face.poster_asset_key(), poster, poster_bytes),
        ] {
            let actual = super::load_asset(&restarted, root.path(), &key, "not-a-download-url")
                .await
                .unwrap();
            assert_eq!(actual, expected);
            assert_eq!(std::fs::read(actual).unwrap(), bytes);
        }
        assert!(
            crate::emoji_assets::cached(root.path(), "face:noto:v3:32:1f914")
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn received_images_reuse_content_before_resolving_any_download_url() {
        use atri_qq::media::{MediaConversation, MediaKind, MediaReference};
        use md5::{Digest, Md5};
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("sent.gif");
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            16,
            12,
            image::Rgba([20, 80, 180, 255]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Gif)
        .unwrap();
        std::fs::write(&source, bytes.get_ref()).unwrap();
        let cache = crate::image_cache::ImageCache::new(root.path());
        let original = cache.remember(&source).await.unwrap();
        let client = atri_qq::Client::new(Default::default(), Default::default()).unwrap();
        let media = atri_qq::media::Media::new(client);
        for (key, conversation, md5) in [
            (
                "preview:dm:other:99",
                MediaConversation::Friend {
                    uid: "other".into(),
                },
                true,
            ),
            (
                "preview:group:123:100",
                MediaConversation::Group { uin: 123 },
                false,
            ),
        ] {
            let mut reference = MediaReference::new(MediaKind::Image, conversation);
            if md5 {
                reference.md5 = Md5::digest(bytes.get_ref()).to_vec();
            } else {
                reference.sha1 = sha1_smol::Sha1::from(bytes.get_ref())
                    .digest()
                    .bytes()
                    .to_vec();
            }
            // No URL or MsgInfo: asking the protocol to resolve this would fail.
            let path = super::load_image(&media, &cache, &reference, root.path(), key)
                .await
                .unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), *bytes.get_ref());
            assert!(path.is_file());
        }
        assert!(original.is_file());
    }
    #[tokio::test]
    async fn concurrent_messages_with_the_same_hash_download_only_once() {
        use atri_qq::media::{MediaConversation, MediaKind, MediaReference};
        use md5::{Digest, Md5};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            16,
            12,
            image::Rgba([20, 80, 180, 255]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let bytes = bytes.into_inner();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let response = bytes.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                    assert!(request.len() < 8192);
                }
                count.fetch_add(1, Ordering::SeqCst);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(&response).await.unwrap();
            }
        });
        let root = tempfile::tempdir().unwrap();
        let cache = crate::image_cache::ImageCache::new(root.path());
        let client = atri_qq::Client::new(Default::default(), Default::default()).unwrap();
        let media = atri_qq::media::Media::new(client);
        let mut reference =
            MediaReference::new(MediaKind::Image, MediaConversation::Group { uin: 123 });
        reference.md5 = Md5::digest(&bytes).to_vec();
        reference.download_url = Some(format!("http://{address}/image"));
        let (first, second) = tokio::join!(
            super::load_image(&media, &cache, &reference, root.path(), "message-one"),
            super::load_image(&media, &cache, &reference, root.path(), "message-two"),
        );
        assert_eq!(first.unwrap(), second.unwrap());
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
        // A new cache instance must also hit the persisted content index offline.
        reference.download_url = None;
        super::load_image(
            &media,
            &crate::image_cache::ImageCache::new(root.path()),
            &reference,
            root.path(),
            "after-restart",
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn legacy_images_move_into_images_with_detected_extensions_without_reencoding() {
        use std::hash::{Hash, Hasher};
        for format in [image::ImageFormat::Png, image::ImageFormat::Gif] {
            let directory = tempfile::tempdir().unwrap();
            let key = "preview:group:42:1:0";
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            key.hash(&mut hash);
            let legacy = directory.path().join(format!("{:016x}", hash.finish()));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                16,
                12,
                image::Rgba([20, 80, 180, 255]),
            ))
            .write_to(&mut bytes, format)
            .unwrap();
            std::fs::write(&legacy, bytes.get_ref()).unwrap();
            let path = fetch_image(directory.path(), key, "http://127.0.0.1:1/offline")
                .await
                .unwrap();
            assert_eq!(path.parent().unwrap(), directory.path().join("images"));
            assert_eq!(path.extension().unwrap(), format.extensions_str()[0]);
            assert!(!legacy.exists());
            assert_eq!(std::fs::read(&path).unwrap(), *bytes.get_ref());
            assert_eq!(
                fetch_image(directory.path(), key, "http://127.0.0.1:1/offline")
                    .await
                    .unwrap(),
                path
            );
        }
    }
}

#[cfg(test)]
mod mini_app_card_tests {
    use super::*;
    use atri_qq::{
        mini_app::ADAPT_SHARE_INFO_COMMAND,
        proto::{messages as rpc, mini_app as pb},
    };
    use bytes::Bytes;
    use prost::Message as _;
    use std::sync::Mutex;

    struct Mock {
        response: pb::AdaptShareInfoResponse,
        calls: Mutex<Vec<(String, Bytes)>>,
    }
    #[async_trait::async_trait]
    impl ServiceClient for Mock {
        async fn request(
            &self,
            command: &str,
            body: Bytes,
            request_type: atri_qq::RequestType,
            encryption: atri_qq::Encryption,
            _: Duration,
        ) -> atri_qq::Result<Bytes> {
            assert_eq!(request_type, atri_qq::RequestType::D2);
            assert_eq!(encryption, atri_qq::Encryption::D2);
            self.calls.lock().unwrap().push((command.into(), body));
            match command {
                ADAPT_SHARE_INFO_COMMAND => Ok(self.response.encode_to_vec().into()),
                "MessageSvc.PbSendMsg" => Ok(rpc::SendMessageResponse {
                    sequence: Some(123),
                    ..Default::default()
                }
                .encode_to_vec()
                .into()),
                _ => panic!("Unexpected request: {command}"),
            }
        }
        async fn snapshot(&self) -> atri_qq::SessionState {
            atri_qq::SessionState {
                profile: atri_qq::Profile::linux(),
                keystore: Keystore::default(),
                platform: atri_qq::PlatformState::Linux {
                    machine_id: String::new(),
                },
                online: true,
            }
        }
        async fn replace_state(&self, _: atri_qq::SessionState) {
            unreachable!()
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<atri_qq::IncomingPacket> {
            tokio::sync::broadcast::channel(1).1
        }
    }
    fn services(response: pb::AdaptShareInfoResponse) -> (Arc<Mock>, MiniApp, Messaging) {
        let client = Arc::new(Mock {
            response,
            calls: Mutex::default(),
        });
        let media = Arc::new(Media::new(client.clone()));
        (
            client.clone(),
            MiniApp::new(client.clone()),
            Messaging::new(client, media),
        )
    }
    fn request() -> MiniAppShareRequest {
        MiniAppShareRequest {
            app_id: "caller-supplied-id".into(),
            title: Some(String::new()),
            description: None,
            is_wx_mini_app: true,
            template: Some(atri_qq::mini_app::MiniAppShareTemplate {
                id: "caller-template".into(),
                data: "{ \"nested\": true }".into(),
            }),
            ..Default::default()
        }
    }
    #[tokio::test]
    async fn mini_app_card_adapts_before_sending_and_preserves_server_authorization() {
        let raw = r#"{"appName":"com.tencent.miniapp_01","appView":"detail_1","metaData":{"detail_1":{"title":"server title"}},"config":{"token":"synthetic-server-token","ctime":1800000000,"unknown":[1,2]},"unknownRoot":{"keep":true}}"#;
        let (client, api, messaging) = services(pb::AdaptShareInfoResponse {
            code: Some(0),
            content: Some(pb::AdaptShareInfoContent {
                json: Some(raw.into()),
            }),
            ..Default::default()
        });
        let conversation = Conversation::Group { uin: 42 };
        let message = send_mini_app_card(&messaging, &api, conversation.clone(), request(), 987)
            .await
            .unwrap();
        assert_eq!(message.random, 987);
        let calls = client.calls.lock().unwrap();
        assert_eq!(
            calls.iter().map(|call| call.0.as_str()).collect::<Vec<_>>(),
            [ADAPT_SHARE_INFO_COMMAND, "MessageSvc.PbSendMsg"]
        );
        let adapted = pb::AdaptShareInfoRequest::decode(calls[0].1.clone())
            .unwrap()
            .body
            .unwrap();
        assert_eq!(adapted.app_id.as_deref(), Some("caller-supplied-id"));
        assert_eq!(adapted.title.as_deref(), Some(""));
        assert!(adapted.description.is_none());
        assert_eq!(adapted.business_type, Some(2));
        let template = adapted.template.unwrap();
        assert_eq!(template.id.as_deref(), Some("caller-template"));
        assert_eq!(template.data.as_deref(), Some("{ \"nested\": true }"));
        let sent = rpc::SendMessageRequest::decode(calls[1].1.clone()).unwrap();
        assert_eq!(sent.random, Some(987));
        let decoded = atri_qq::messages::decode_elements(&sent.body, &conversation, 0).unwrap();
        let [MessageElement::LightApp(json)] = decoded.elements.as_slice() else {
            panic!("expected a server-generated LightApp")
        };
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let original: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(value["config"], original["config"]);
        assert_eq!(value["unknownRoot"], original["unknownRoot"]);
        assert_eq!(value["meta"], original["metaData"]);
        assert_eq!(value["miniappShareOrigin"], 3);
        assert_eq!(value["miniappOpenRefer"], "10002");
        assert!(
            matches!(message.elements.as_slice(), [MessageElement::LightApp(sent)] if sent == json)
        );
    }
    #[tokio::test]
    async fn mini_app_card_rejection_and_malformed_success_never_send_a_message() {
        for response in [
            pb::AdaptShareInfoResponse {
                code: Some(403),
                error_message: Some("synthetic rejection".into()),
                ..Default::default()
            },
            pb::AdaptShareInfoResponse::default(),
            pb::AdaptShareInfoResponse {
                code: Some(0),
                content: Some(pb::AdaptShareInfoContent {
                    json: Some("{}".into()),
                }),
                ..Default::default()
            },
        ] {
            let (client, api, messaging) = services(response);
            assert!(
                send_mini_app_card(
                    &messaging,
                    &api,
                    Conversation::Group { uin: 42 },
                    request(),
                    987
                )
                .await
                .is_err()
            );
            let calls = client.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, ADAPT_SHARE_INFO_COMMAND);
        }
        let (client, api, messaging) = services(Default::default());
        assert!(
            send_mini_app_card(
                &messaging,
                &api,
                Conversation::Group { uin: 42 },
                MiniAppShareRequest::default(),
                987
            )
            .await
            .is_err()
        );
        assert!(client.calls.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod music_share_tests {
    use super::*;
    use atri_qq::proto::sdk_share as pb;
    use bytes::Bytes;
    use prost::Message as _;
    use std::sync::Mutex;

    struct Mock {
        calls: Mutex<Vec<(String, Bytes)>>,
        reject: bool,
        timeout: bool,
    }
    #[async_trait::async_trait]
    impl ServiceClient for Mock {
        async fn request(
            &self,
            command: &str,
            body: Bytes,
            kind: atri_qq::RequestType,
            encryption: atri_qq::Encryption,
            _: Duration,
        ) -> atri_qq::Result<Bytes> {
            assert_eq!(kind, atri_qq::RequestType::D2);
            assert_eq!(encryption, atri_qq::Encryption::D2);
            assert_eq!(command, atri_qq::sdk_share::SHARE_MUSIC_COMMAND);
            self.calls.lock().unwrap().push((command.into(), body));
            if self.timeout {
                return Err(atri_qq::Error::Timeout);
            }
            Ok(pb::LegacyOidbEnvelope {
                command: Some(0xb77),
                service: Some(9),
                result: Some(if self.reject { 403 } else { 0 }),
                error_message: self.reject.then(|| "rejected".into()),
                body: Some(
                    pb::ShareMusicResponse {
                        wording: Some("ok".into()),
                        ..Default::default()
                    }
                    .encode_to_vec()
                    .into(),
                ),
                ..Default::default()
            }
            .encode_to_vec()
            .into())
        }
        async fn snapshot(&self) -> atri_qq::SessionState {
            atri_qq::SessionState {
                profile: atri_qq::Profile::linux(),
                keystore: Keystore::default(),
                platform: atri_qq::PlatformState::Linux {
                    machine_id: String::new(),
                },
                online: true,
            }
        }
        async fn replace_state(&self, _: atri_qq::SessionState) {
            unreachable!()
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<atri_qq::IncomingPacket> {
            tokio::sync::broadcast::channel(1).1
        }
    }
    fn source() -> AndroidShareSource {
        AndroidShareSource {
            app_id: 12345,
            package_name: "dev.example.music".into(),
            signature_md5: "0123456789ABCDEF0123456789ABCDEF".into(),
            sdk_version: None,
        }
    }
    fn content() -> MusicShareContent {
        MusicShareContent {
            title: "Song".into(),
            summary: Some("Artist".into()),
            jump_url: "https://example.com/song".into(),
            picture_url: "https://example.com/cover.jpg".into(),
            music_url: "https://example.com/song.mp3".into(),
        }
    }
    #[tokio::test]
    async fn music_share_uses_sdk_rpc_once_for_friend_and_group() {
        for (conversation, send_type) in [
            (Conversation::Group { uin: 42 }, 1),
            (
                Conversation::Friend {
                    uin: 42,
                    uid: "peer".into(),
                },
                0,
            ),
        ] {
            let client = Arc::new(Mock {
                calls: Mutex::default(),
                reject: false,
                timeout: false,
            });
            let api = SdkShare::new(client.clone());
            let operations = Operations::new(client.clone());
            let receipt = share_music(&api, &operations, conversation, source(), content())
                .await
                .unwrap();
            assert_eq!(receipt.wording.as_deref(), Some("ok"));
            let calls = client.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            let envelope = pb::LegacyOidbEnvelope::decode(calls[0].1.clone()).unwrap();
            let body = pb::ShareMusicRequest::decode(envelope.body.unwrap()).unwrap();
            assert_eq!(body.send_type, Some(send_type));
            assert_eq!(body.receiver_uin, Some(42));
            assert_eq!(body.app_id, Some(12345));
            let info = body.client_info.unwrap();
            assert_eq!(
                info.android_package_name.as_deref(),
                Some("dev.example.music")
            );
            assert_eq!(
                info.android_signature.as_deref(),
                Some("0123456789abcdef0123456789abcdef")
            );
            let music = body.rich_message.unwrap();
            assert_eq!(music.title.as_deref(), Some("Song"));
            assert_eq!(music.summary.as_deref(), Some("Artist"));
            assert_eq!(
                music.music_url.as_deref(),
                Some("https://example.com/song.mp3")
            );
            assert_eq!(
                music.picture_url.as_deref(),
                Some("https://example.com/cover.jpg")
            );
            assert_eq!(music.url.as_deref(), Some("https://example.com/song"));
        }
    }
    #[tokio::test]
    async fn rejected_music_share_is_not_retried_or_sent_as_a_normal_message() {
        let client = Arc::new(Mock {
            calls: Mutex::default(),
            reject: true,
            timeout: false,
        });
        let error = share_music(
            &SdkShare::new(client.clone()),
            &Operations::new(client.clone()),
            Conversation::Group { uin: 42 },
            source(),
            content(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("403"));
        assert_eq!(client.calls.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn unconfirmed_music_share_preserves_uncertainty_without_retrying() {
        let client = Arc::new(Mock {
            calls: Mutex::default(),
            reject: false,
            timeout: true,
        });
        let error = share_music(
            &SdkShare::new(client.clone()),
            &Operations::new(client.clone()),
            Conversation::Group { uin: 42 },
            source(),
            content(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<atri_qq::Error>(),
            Some(atri_qq::Error::UnconfirmedSdkShare { .. })
        ));
        assert_eq!(client.calls.lock().unwrap().len(), 1);
    }
}
