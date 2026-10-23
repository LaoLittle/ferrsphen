use anyhow::Result;
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
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let tx = self.tree.begin()?;
        tx.get(key.as_bytes())?
            .map(|bytes| ciborium::from_reader(bytes.as_slice()).map_err(Into::into))
            .transpose()
    }
    pub async fn put<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let mut bytes = Vec::new();
        ciborium::into_writer(value, &mut bytes)?;
        let mut tx = self.tree.begin()?;
        tx.set(key.as_bytes(), &bytes)?;
        tx.set_durability(Durability::Immediate);
        tx.commit().await?;
        Ok(())
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
            messages.push_front(ciborium::from_reader(value.as_slice())?);
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
                model.receive(ciborium::from_reader(bytes.as_slice())?, false);
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
                .insert(ciborium::from_reader(value.as_slice())?);
            recalls.next()?;
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
