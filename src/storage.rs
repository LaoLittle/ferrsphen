use anyhow::{Context as _, Result};
use atri_qq::{Keystore, messages::Message};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};
use surrealkv::{Durability, LSMIterator, Tree, TreeBuilder};

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub keystore: Keystore,
    pub machine_id: String,
    pub qua: String,
}

pub struct Database {
    tree: Tree,
    pub root: PathBuf,
}
impl Database {
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
        }
        let tree = TreeBuilder::new()
            .with_path(root.join("ferrsphen.skv"))
            .build()?;
        Ok(Self {
            tree,
            root: root.into(),
        })
    }
    pub fn records_with_prefix<T: DeserializeOwned>(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<T>> {
        let tx = self.tree.begin()?;
        let mut upper = prefix.as_bytes().to_vec();
        upper.push(255);
        let mut rows = tx.range(prefix.as_bytes(), upper.as_slice())?;
        rows.seek_last()?;
        let mut result = Vec::new();
        while rows.valid() && result.len() < limit {
            result.push(crate::proto::decode(rows.value()?.as_slice())?);
            rows.prev()?;
        }
        result.reverse();
        Ok(result)
    }
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let tx = self.tree.begin()?;
        tx.get(key.as_bytes())?
            .map(|bytes| crate::proto::decode(bytes.as_slice()))
            .transpose()
    }
    pub async fn delete(&self, key: &str) -> Result<()> {
        let mut tx = self.tree.begin()?;
        tx.delete(key.as_bytes())?;
        tx.set_durability(Durability::Immediate);
        tx.commit().await?;
        Ok(())
    }
    pub async fn purge_telegram_api_credentials(&self) -> Result<usize> {
        let prefix = b"adapter/session/";
        let tx = self.tree.begin()?;
        let mut rows = tx.range(prefix.as_slice(), b"adapter/session0".as_slice())?;
        rows.seek_first()?;
        let mut keys = Vec::new();
        while rows.valid() {
            let key = rows.key().user_key().to_vec();
            if let Some(encoded) = key.strip_prefix(prefix.as_slice())
                && let Ok(decoded) = hex::decode(encoded)
                && let Ok(decoded) = std::str::from_utf8(&decoded)
                && decoded.starts_with("telegram/")
                && decoded.ends_with("/credentials")
            {
                keys.push(key);
            }
            rows.next()?;
        }
        drop(rows);
        drop(tx);
        if keys.is_empty() {
            return Ok(0);
        }
        let mut tx = self.tree.begin()?;
        for key in &keys {
            tx.delete(key)?;
        }
        tx.set_durability(Durability::Immediate);
        tx.commit().await?;
        Ok(keys.len())
    }
    pub async fn save_session(&self, session: &Session) -> Result<()> {
        let bytes = crate::proto::encode_session(session)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        use std::io::Write as _;
        file.write_all(&crate::proto::encode_keystore_file(session))?;
        file.as_file().sync_all()?;
        file.persist(self.root.join("keystore.bin"))?;
        #[cfg(unix)]
        std::fs::File::open(&self.root)?.sync_all()?;
        self.put_bytes("session/active", &bytes).await
    }
    pub async fn put<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let bytes = crate::proto::encode(value)?;
        self.put_bytes(key, &bytes).await
    }
    async fn put_bytes(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let mut tx = self.tree.begin()?;
        tx.set(key.as_bytes(), bytes)?;
        tx.set_durability(Durability::Immediate);
        tx.commit().await?;
        Ok(())
    }
    pub async fn migrate_legacy(&self) -> Result<usize> {
        let tx = self.tree.begin()?;
        let mut rows = tx.range(b"".as_slice(), b"\xff".as_slice())?;
        rows.seek_first()?;
        let mut pending = Vec::new();
        let mut pending_bytes = 0;
        let mut migrated = 0;
        while rows.valid() {
            let bytes = rows.value()?;
            if !crate::proto::is_protobuf(bytes.as_slice()) {
                let key = rows.key().user_key().to_vec();
                let encoded =
                    crate::proto::transcode_legacy(bytes.as_slice()).with_context(|| {
                        format!("迁移数据库记录失败：{}", String::from_utf8_lossy(&key))
                    })?;
                pending_bytes += encoded.len();
                pending.push((key, encoded));
            }
            rows.next()?;
            if pending.len() >= 128 || pending_bytes >= 4 * 1024 * 1024 {
                migrated += self.write_migration_batch(&mut pending).await?;
                pending_bytes = 0;
            }
        }
        migrated += self.write_migration_batch(&mut pending).await?;
        Ok(migrated)
    }
    async fn write_migration_batch(&self, pending: &mut Vec<(Vec<u8>, Vec<u8>)>) -> Result<usize> {
        if pending.is_empty() {
            return Ok(0);
        }
        let count = pending.len();
        let mut tx = self.tree.begin()?;
        for (key, bytes) in pending.iter() {
            tx.set(key, bytes)?;
        }
        tx.set_durability(Durability::Immediate);
        tx.commit().await?;
        pending.clear();
        Ok(count)
    }
    pub fn user_uin(&self, account: u64, uid: &str) -> Result<Option<u64>> {
        Ok(self
            .get::<u64>(&format!("user-uin/{account}/{}", hex::encode(uid)))?
            .filter(|uin| *uin != 0))
    }
    pub async fn save_user_uin(&self, account: u64, uid: &str, uin: u64) -> Result<()> {
        anyhow::ensure!(!uid.is_empty() && uin != 0, "Invalid user identity");
        self.put(&format!("user-uin/{account}/{}", hex::encode(uid)), &uin)
            .await
    }
    pub async fn save_message(&self, account: u64, message: &Message) -> Result<()> {
        if message.is_recalled() {
            self.save_recall(account, &message.conversation, message.sequence)
                .await?;
        }
        let key = format!(
            "message/{account}/{}/{:020}/{:020}/{:020}",
            hex::encode(crate::model::conversation_key(&message.conversation).as_bytes()),
            message.time,
            message.sequence,
            message.message_id
        );
        self.put(&key, message).await
    }
    pub fn messages(&self, account: u64, key: &str, limit: usize) -> Result<Vec<Message>> {
        let prefix = format!("message/{account}/{}/", hex::encode(key.as_bytes()));
        let tx = self.tree.begin()?;
        let mut upper = prefix.as_bytes().to_vec();
        upper.push(255);
        let mut iter = tx.range(prefix.as_bytes(), upper.as_slice())?;
        iter.seek_last()?;
        let mut messages = std::collections::VecDeque::new();
        while iter.valid() && messages.len() < limit {
            let value = iter.value()?;
            messages.push_front(decode_stored_message(value.as_slice(), account)?);
            iter.prev()?;
        }
        Ok(messages.into_iter().collect())
    }
    pub async fn save_recall(
        &self,
        account: u64,
        conversation: &atri_qq::messages::Conversation,
        sequence: u64,
    ) -> Result<()> {
        let identity = format!(
            "{}:{sequence}",
            crate::model::conversation_key(conversation)
        );
        self.put(
            &format!("recall/{account}/{}", hex::encode(identity.as_bytes())),
            &identity,
        )
        .await
    }
    pub async fn save_notification(
        &self,
        account: u64,
        event: &atri_qq::messages::PushEvent,
    ) -> Result<crate::activity::ActivityRecord> {
        let record = crate::activity::ActivityRecord::notification(event.clone());
        self.put(&format!("notification/{account}/{}", record.id), event)
            .await?;
        Ok(record)
    }
    pub async fn save_activity(
        &self,
        account: u64,
        record: &crate::activity::ActivityRecord,
    ) -> Result<()> {
        self.put(&format!("activity/{account}/{}", record.id), record)
            .await
    }
    pub fn activity(&self, account: u64) -> Result<Vec<crate::activity::ActivityRecord>> {
        use crate::activity::{ActivityEvent, ActivityRecord};
        use atri_qq::messages::PushEvent;
        let mut records = Vec::new();
        for namespace in ["notification", "activity"] {
            let prefix = format!("{namespace}/{account}/");
            let mut upper = prefix.as_bytes().to_vec();
            upper.push(255);
            let tx = self.tree.begin()?;
            let mut rows = tx.range(prefix.as_bytes(), upper.as_slice())?;
            rows.seek_first()?;
            while rows.valid() {
                let bytes = rows.value()?;
                if namespace == "activity" {
                    records.push(crate::proto::decode(bytes.as_slice())?);
                } else {
                    let event: PushEvent = crate::proto::decode(bytes.as_slice())?;
                    if matches!(event, PushEvent::Poke { .. } | PushEvent::Reaction { .. }) {
                        let key = rows.key().user_key().to_vec();
                        let id = std::str::from_utf8(&key[prefix.len()..])?.to_owned();
                        let stamp = id
                            .split('-')
                            .next()
                            .unwrap_or_default()
                            .parse::<u64>()
                            .unwrap_or(0);
                        let time = if stamp > 10_000_000_000_000_000 {
                            stamp / 1_000_000
                        } else {
                            stamp
                        };
                        records.push(ActivityRecord {
                            id,
                            time: time.min(i64::MAX as u64) as i64,
                            event: ActivityEvent::Notification(event),
                        });
                    }
                }
                rows.next()?;
            }
        }
        records.sort_by(|a, b| (a.time, &a.id).cmp(&(b.time, &b.id)));
        Ok(records)
    }
    pub fn restore(&self, account: u64) -> Result<crate::model::Model> {
        use crate::model::{Model, conversation_key};
        let mut model = Model {
            chats: self.get(&format!("chats/{account}"))?.unwrap_or_default(),
            ..Model::default()
        };
        model.contacts(
            self.get(&format!("friends/{account}"))?.unwrap_or_default(),
            self.get(&format!("groups/{account}"))?.unwrap_or_default(),
        );
        let tx = self.tree.begin()?;
        let prefix = format!("message/{account}/").into_bytes();
        let mut upper = prefix.clone();
        upper.push(255);
        let mut conversations = tx.range(&prefix, &upper)?;
        conversations.seek_first()?;
        while conversations.valid() {
            let key = conversations.key().user_key().to_vec();
            let end = key[prefix.len()..]
                .iter()
                .position(|byte| *byte == b'/')
                .ok_or_else(|| anyhow::anyhow!("消息索引无效"))?
                + prefix.len()
                + 1;
            let message_prefix = &key[..end];
            let mut next = message_prefix.to_vec();
            next.push(255);
            let mut rows = tx.range(message_prefix, &next)?;
            rows.seek_last()?;
            for _ in 0..300 {
                if !rows.valid() {
                    break;
                }
                let bytes = rows.value()?;
                model.receive(decode_stored_message(bytes.as_slice(), account)?, false);
                rows.prev()?;
            }
            conversations.seek(&next)?;
        }
        let prefix = format!("recall/{account}/").into_bytes();
        let mut upper = prefix.clone();
        upper.push(255);
        let mut recalls = tx.range(&prefix, &upper)?;
        recalls.seek_first()?;
        while recalls.valid() {
            let value = recalls.value()?;
            model
                .recalled
                .insert(crate::proto::decode(value.as_slice())?);
            recalls.next()?;
        }
        for record in self.activity(account)? {
            model.apply_activity(record);
        }
        model.chats.sort_by_key(|chat| {
            std::cmp::Reverse(
                model
                    .messages
                    .get(&conversation_key(&chat.conversation))
                    .and_then(|rows| rows.last())
                    .map_or(0, |message| message.time),
            )
        });
        model.selected = model
            .chats
            .first()
            .map(|chat| conversation_key(&chat.conversation));
        for group in &model.groups {
            if let Some(members) = self.get(&format!("members/{account}/{}", group.uin))? {
                model.members.insert(group.uin, members);
            }
        }
        Ok(model)
    }
    pub fn session(&self) -> Result<Option<Session>> {
        let keyfile = self.root.join("keystore.bin");
        match std::fs::read(&keyfile) {
            Ok(bytes) => {
                return crate::proto::decode_keystore_file(&bytes)
                    .with_context(|| format!("读取 {} 失败", keyfile.display()))
                    .map(Some);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("读取 {} 失败", keyfile.display()));
            }
        }
        if let Some(session) = self.get("session/active")? {
            return Ok(Some(session));
        }
        let keyfile = self.root.join("storage/io.atri.qq/data/keystore.cbor");
        let configfile = self.root.join("storage/io.atri.qq/config/config.toml");
        if keyfile.is_file() && configfile.is_file() {
            #[derive(Deserialize)]
            struct Legacy {
                machine_id: String,
                qua: String,
            }
            let config: Legacy = toml::from_str(&std::fs::read_to_string(configfile)?)?;
            let keystore = ciborium::from_reader(std::fs::File::open(keyfile)?)?;
            return Ok(Some(Session {
                keystore,
                machine_id: config.machine_id,
                qua: config.qua,
            }));
        }
        Ok(None)
    }
    pub async fn close(&self) -> Result<()> {
        self.tree.close().await?;
        Ok(())
    }
}

fn decode_stored_message(bytes: &[u8], account: u64) -> Result<Message> {
    use atri_qq::{
        messages::decode_elements,
        proto::{decode, messages::CommonMessage},
    };
    let mut message: Message = crate::proto::decode(bytes)?;
    let Ok(common) = decode::<CommonMessage>(message.raw.as_slice()) else {
        return Ok(message);
    };
    if common.message_body.is_empty() {
        return Ok(message);
    }
    match decode_elements(&common.message_body, &message.conversation, account) {
        Ok(content) => {
            message.elements = content.elements;
            message.metadata = content.metadata;
        }
        Err(error) => tracing::warn!(
            ?error,
            sequence = message.sequence,
            "refresh cached message failed"
        ),
    }
    Ok(message)
}

/// Remove the old application credential records from both managed and legacy account stores.
pub async fn purge_legacy_telegram_credentials(root: &Path) -> Result<usize> {
    let mut roots = vec![root.to_path_buf(), root.join("adapters")];
    let accounts = root.join("accounts");
    if accounts.is_dir() {
        for entry in std::fs::read_dir(accounts)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                roots.push(entry.path());
            }
        }
    }
    let mut removed = 0;
    for root in roots {
        if !root.join("ferrsphen.skv").is_dir() {
            continue;
        }
        let db = Database::open(&root)?;
        let result = db.purge_telegram_api_credentials().await;
        db.close().await?;
        removed += result?;
    }
    Ok(removed)
}

#[cfg(test)]
mod migration_tests {
    use super::{Database, Session};
    use atri_qq::{
        Keystore,
        messages::{Conversation, Message, MessageElement, PushEvent},
    };
    use serde::Serialize;
    async fn legacy<T: Serialize>(db: &Database, key: &str, value: &T) {
        let mut bytes = Vec::new();
        ciborium::into_writer(value, &mut bytes).unwrap();
        let mut tx = db.tree.begin().unwrap();
        tx.set(key.as_bytes(), &bytes).unwrap();
        tx.commit().await.unwrap();
    }
    #[tokio::test]
    async fn typed_session_preserves_tickets_device_identity_and_optional_o3_policy() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open(root.path()).unwrap();
        let session = Session {
            keystore: Keystore {
                uin: u64::MAX,
                uid: "account-uid".into(),
                a1: (0..=255).collect(),
                a2: vec![255, 0, 128],
                d2_key: vec![3; 16],
                ps_keys: std::collections::BTreeMap::from([("qq.com".into(), "ticket".into())]),
                o3_xwid_switch: Some(0),
                qr_sig: vec![5; 16],
                ios_msf_imei: "12A3B456-7890-4ABC-8DEF-123456789ABC".into(),
                key_exchange_session: Some(
                    atri_qq::NtKeyExchangeSession::new(
                        atri_qq::ClientPlatform::IPhone,
                        bytes::Bytes::from_static(&[7; 16]),
                        bytes::Bytes::from_static(b"synthetic-session-ticket"),
                        600,
                    )
                    .unwrap(),
                ),
                login_cookie: vec![0xff, 0x00, 0xc0, 0x80],
                ..Default::default()
            },
            machine_id: "machine".into(),
            qua: atri_qq::Profile::linux().qua,
        };
        for policy in [Some(0), Some(u32::MAX), None] {
            let mut expected = session.clone();
            expected.keystore.o3_xwid_switch = policy;
            db.save_session(&expected).await.unwrap();
            let loaded = db.session().unwrap().unwrap();
            assert_eq!(
                serde_json::to_value(&loaded).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
            assert!(loaded.keystore.qr_sig.is_empty());
            assert!(loaded.keystore.key_exchange_session.is_none());
            assert!(loaded.keystore.login_cookie.is_empty());
        }
        let keyfile = root.path().join("keystore.bin");
        use prost::Message as _;
        let file = crate::proto::session::KeystoreFile::decode(
            std::fs::read(&keyfile).unwrap().as_slice(),
        )
        .unwrap();
        assert_eq!(file.schema_version, 1);
        assert_eq!(file.session.unwrap().keystore.unwrap().uin, u64::MAX);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&keyfile).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(db.migrate_legacy().await.unwrap(), 0);
        let mut tx = db.tree.begin().unwrap();
        tx.delete(b"session/active").unwrap();
        tx.commit().await.unwrap();
        db.close().await.unwrap();
        drop(db);
        let db = Database::open(root.path()).unwrap();
        let loaded = db.session().unwrap().unwrap();
        assert_eq!(loaded.keystore.a1, session.keystore.a1);
        assert_eq!(loaded.keystore.guid, session.keystore.guid);
        assert_eq!(loaded.keystore.device_name, session.keystore.device_name);
        assert_eq!(loaded.keystore.ios_msf_imei, session.keystore.ios_msf_imei);
        assert_eq!(loaded.machine_id, session.machine_id);
        assert_eq!(loaded.qua, session.qua);
        db.close().await.unwrap();
    }
    #[tokio::test]
    async fn corrupt_keystore_file_does_not_silently_fall_back_to_stale_credentials() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open(root.path()).unwrap();
        let session = Session {
            keystore: Keystore::default(),
            machine_id: "machine".into(),
            qua: "profile".into(),
        };
        db.save_session(&session).await.unwrap();
        std::fs::write(root.path().join("keystore.bin"), [255, 0]).unwrap();
        assert!(db.session().is_err());
        db.close().await.unwrap();
    }
    #[tokio::test]
    async fn legacy_keystore_file_can_be_imported_once_and_reopened_from_protobuf() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("storage/io.atri.qq");
        std::fs::create_dir_all(directory.join("data")).unwrap();
        std::fs::create_dir_all(directory.join("config")).unwrap();
        let keyfile = directory.join("data/keystore.cbor");
        let keystore = Keystore {
            uin: 12345,
            a2: vec![255, 0, 42],
            o3_xwid_switch: Some(0),
            ..Default::default()
        };
        ciborium::into_writer(&keystore, std::fs::File::create(&keyfile).unwrap()).unwrap();
        std::fs::write(
            directory.join("config/config.toml"),
            "machine_id = \"machine\"\nqua = \"test-qua\"\n",
        )
        .unwrap();
        let db = Database::open(root.path()).unwrap();
        let imported = db.session().unwrap().unwrap();
        db.save_session(&imported).await.unwrap();
        db.close().await.unwrap();
        drop(db);
        std::fs::remove_dir_all(directory).unwrap();
        let db = Database::open(root.path()).unwrap();
        let loaded = db.session().unwrap().unwrap();
        assert_eq!(loaded.keystore.uin, keystore.uin);
        assert_eq!(loaded.keystore.a2, keystore.a2);
        assert_eq!(loaded.keystore.guid, keystore.guid);
        assert_eq!(loaded.keystore.o3_xwid_switch, Some(0));
        assert_eq!(loaded.machine_id, "machine");
        db.close().await.unwrap();
    }
    #[tokio::test]
    async fn legacy_sessions_messages_pokes_and_reactions_migrate_without_changing_account_keys() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open(root.path()).unwrap();
        let session = Session {
            keystore: Keystore {
                uin: 1,
                uid: "self".into(),
                o3_xwid_switch: Some(0),
                a1: vec![0, 255],
                ..Default::default()
            },
            machine_id: "machine".into(),
            qua: atri_qq::Profile::linux().qua,
        };
        legacy(&db, "session/active", &session).await;
        let message = Message {
            conversation: Conversation::Group { uin: 42 },
            sender_uin: 7,
            sender_uid: "peer".into(),
            recipient_uin: 1,
            recipient_uid: "self".into(),
            sender_name: "昵称".into(),
            sequence: u64::MAX,
            client_sequence: 5,
            message_id: 9,
            random: 4,
            time: 1_700_000_000,
            is_self: false,
            is_bot: Some(true),
            elements: vec![MessageElement::Text("文本🌙".into())],
            metadata: vec![],
            raw: vec![255, 0, 128],
        };
        let message_key = format!(
            "message/1/{}/{:020}/{:020}/{:020}",
            hex::encode("group:42"),
            message.time,
            message.sequence,
            message.message_id
        );
        legacy(&db, &message_key, &message).await;
        legacy(&db, "recall/1/6d61726b6572", &"group:42:9").await;
        legacy(&db, "user-uin/1/70656572", &u64::MAX).await;
        let poke = PushEvent::Poke {
            group_uin: None,
            actor_uin: Some(7),
            target_uin: Some(1),
            parameters: Default::default(),
            raw: vec![255, 1],
        };
        legacy(&db, "notification/1/00000001700000000100", &poke).await;
        let reaction = PushEvent::Reaction {
            group_uin: 42,
            sequence: u64::MAX,
            operator_uid: "self".into(),
            added: true,
            code: "76".into(),
            count: 2,
        };
        legacy(&db, "notification/1/00000001700000000200", &reaction).await;
        let confirmed = crate::activity::ActivityRecord::new(
            crate::activity::ActivityEvent::ReactionConfirmed {
                group: 42,
                sequence: u64::MAX,
                code: "76".into(),
                added: true,
                operator_uid: "self".into(),
            },
        );
        legacy(&db, "activity/1/local", &confirmed).await;
        legacy(&db, "friends/2", &Vec::<atri_qq::contacts::Friend>::new()).await;
        assert_eq!(
            db.session().unwrap().unwrap().keystore.o3_xwid_switch,
            Some(0)
        );
        assert_eq!(db.migrate_legacy().await.unwrap(), 8);
        assert_eq!(db.migrate_legacy().await.unwrap(), 0);
        let tx = db.tree.begin().unwrap();
        assert!(crate::proto::is_protobuf(
            &tx.get(message_key.as_bytes()).unwrap().unwrap()
        ));
        drop(tx);
        db.close().await.unwrap();
        drop(db);
        let db = Database::open(root.path()).unwrap();
        let loaded = db.session().unwrap().unwrap();
        assert_eq!(loaded.keystore.a1, vec![0, 255]);
        assert_eq!(loaded.keystore.o3_xwid_switch, Some(0));
        let rows = db.messages(1, "group:42", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sequence, u64::MAX);
        assert_eq!(rows[0].raw, message.raw);
        assert_eq!(crate::model::summary(&rows[0].elements), "文本🌙");
        assert_eq!(db.user_uin(1, "peer").unwrap(), Some(u64::MAX));
        let restored = db.restore(1).unwrap();
        assert!(restored.recalled.contains("group:42:9"));
        assert_eq!(restored.activity.len(), 3);
        assert!(restored.activity.iter().any(|record| matches!(
            record.event,
            crate::activity::ActivityEvent::Notification(PushEvent::Poke {
                group_uin: None,
                ..
            })
        )));
        assert_eq!(restored.reactions[&(42, u64::MAX, "76".into())].count, 2);
        assert!(db.restore(2).unwrap().activity.is_empty());
        db.close().await.unwrap();
    }
    #[tokio::test]
    async fn migration_handles_batches_mixed_formats_and_resumes_without_rewriting_protobuf() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open(root.path()).unwrap();
        let mut tx = db.tree.begin().unwrap();
        for i in 0..300u64 {
            let mut bytes = Vec::new();
            ciborium::into_writer(&i, &mut bytes).unwrap();
            tx.set(format!("legacy/{i:03}").as_bytes(), &bytes).unwrap();
        }
        tx.commit().await.unwrap();
        db.put("new/one", &u64::MAX).await.unwrap();
        let before = db.tree.begin().unwrap().get(b"new/one").unwrap().unwrap();
        assert_eq!(db.migrate_legacy().await.unwrap(), 300);
        let after = db.tree.begin().unwrap().get(b"new/one").unwrap().unwrap();
        assert_eq!(before, after);
        for i in 0..300u64 {
            assert_eq!(db.get::<u64>(&format!("legacy/{i:03}")).unwrap(), Some(i));
        }
        legacy(&db, "legacy/extra", &123u64).await;
        assert_eq!(db.migrate_legacy().await.unwrap(), 1);
        assert_eq!(db.migrate_legacy().await.unwrap(), 0);
        db.close().await.unwrap();
    }
    #[tokio::test]
    async fn invalid_legacy_record_fails_without_overwriting_the_original() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open(root.path()).unwrap();
        let invalid = [0xff, 0xff];
        let mut tx = db.tree.begin().unwrap();
        tx.set(b"broken", &invalid).unwrap();
        tx.commit().await.unwrap();
        assert!(db.migrate_legacy().await.is_err());
        let bytes = db.tree.begin().unwrap().get(b"broken").unwrap().unwrap();
        assert_eq!(bytes.as_slice(), invalid);
        db.close().await.unwrap();
    }
}

#[cfg(test)]
mod credential_cleanup_tests {
    use super::*;
    #[tokio::test]
    async fn startup_purges_credentials_from_legacy_active_and_unfinished_accounts_only() {
        let root = tempfile::tempdir().unwrap();
        let directories = [
            root.path().to_path_buf(),
            root.path().join("adapters"),
            root.path().join("accounts/active"),
            root.path().join("accounts/unfinished"),
        ];
        let credential_key = format!(
            "adapter/session/{}",
            hex::encode("telegram/6f6e65/credentials")
        );
        let session_key = format!("adapter/session/{}", hex::encode("telegram/6f6e65/session"));
        let other_key = format!("adapter/session/{}", hex::encode("matrix/6f6e65/session"));
        for directory in &directories {
            let db = Database::open(directory).unwrap();
            db.put(
                &credential_key,
                &serde_json::json!({"api_id":12345,"api_hash":"synthetic-obsolete-hash"}),
            )
            .await
            .unwrap();
            db.put(
                &session_key,
                &serde_json::json!({"home_dc":2,"auth_key":"retained-session"}),
            )
            .await
            .unwrap();
            db.put(
                &other_key,
                &serde_json::json!({"access_token":"retained-matrix-session"}),
            )
            .await
            .unwrap();
            db.close().await.unwrap();
        }
        assert_eq!(
            purge_legacy_telegram_credentials(root.path())
                .await
                .unwrap(),
            directories.len()
        );
        for directory in &directories {
            let db = Database::open(directory).unwrap();
            assert!(
                db.get::<serde_json::Value>(&credential_key)
                    .unwrap()
                    .is_none()
            );
            assert!(db.get::<serde_json::Value>(&session_key).unwrap().is_some());
            assert!(db.get::<serde_json::Value>(&other_key).unwrap().is_some());
            db.close().await.unwrap();
        }
        assert_eq!(
            purge_legacy_telegram_credentials(root.path())
                .await
                .unwrap(),
            0
        );
    }
}
