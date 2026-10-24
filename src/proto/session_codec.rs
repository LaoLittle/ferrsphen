use super::session as wire;
use crate::storage::Session;
use anyhow::{Context as _, Result};
use atri_qq::Keystore;

impl From<&Session> for wire::Session {
    fn from(value: &Session) -> Self {
        Self {
            keystore: Some(wire::Keystore::from(&value.keystore)),
            machine_id: value.machine_id.clone(),
            qua: value.qua.clone(),
        }
    }
}
impl TryFrom<wire::Session> for Session {
    type Error = anyhow::Error;
    fn try_from(value: wire::Session) -> Result<Self> {
        Ok(Self {
            keystore: value
                .keystore
                .context("Session is missing its keystore")?
                .try_into()?,
            machine_id: value.machine_id,
            qua: value.qua,
        })
    }
}
impl From<&Keystore> for wire::Keystore {
    fn from(value: &Keystore) -> Self {
        Self {
            uin: value.uin,
            uid: value.uid.clone(),
            guid: value.guid.clone(),
            device_name: value.device_name.clone(),
            android_id: value.android_id.clone(),
            qimei: value.qimei.clone(),
            nickname: value.nickname.clone(),
            age: u32::from(value.age),
            gender: u32::from(value.gender),
            face_id: u32::from(value.face_id),
            a1: value.a1.clone(),
            a1_key: value.a1_key.clone(),
            a2: value.a2.clone(),
            a2_key: value.a2_key.clone(),
            d2: value.d2.clone(),
            d2_key: value.d2_key.clone(),
            tgtgt_key: value.tgtgt_key.clone(),
            no_pic_sig: value.no_pic_sig.clone(),
            random_key: value.random_key.clone(),
            ksid: value.ksid.clone(),
            s_key: value.s_key.clone(),
            super_key: value.super_key.clone(),
            st: value.st.clone(),
            st_key: value.st_key.clone(),
            st_web: value.st_web.clone(),
            wt_session_ticket: value.wt_session_ticket.clone(),
            wt_session_ticket_key: value.wt_session_ticket_key.clone(),
            ps_keys: value
                .ps_keys
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            o3_xwid_switch: value.o3_xwid_switch,
        }
    }
}
impl TryFrom<wire::Keystore> for Keystore {
    type Error = anyhow::Error;
    fn try_from(value: wire::Keystore) -> Result<Self> {
        Ok(Self {
            uin: value.uin,
            uid: value.uid,
            guid: value.guid,
            device_name: value.device_name,
            android_id: value.android_id,
            qimei: value.qimei,
            nickname: value.nickname,
            age: value.age.try_into().context("Invalid keystore age")?,
            gender: value.gender.try_into().context("Invalid keystore gender")?,
            face_id: value
                .face_id
                .try_into()
                .context("Invalid keystore face_id")?,
            a1: value.a1,
            a1_key: value.a1_key,
            a2: value.a2,
            a2_key: value.a2_key,
            d2: value.d2,
            d2_key: value.d2_key,
            tgtgt_key: value.tgtgt_key,
            no_pic_sig: value.no_pic_sig,
            random_key: value.random_key,
            ksid: value.ksid,
            s_key: value.s_key,
            super_key: value.super_key,
            st: value.st,
            st_key: value.st_key,
            st_web: value.st_web,
            wt_session_ticket: value.wt_session_ticket,
            wt_session_ticket_key: value.wt_session_ticket_key,
            ps_keys: value.ps_keys.into_iter().collect(),
            o3_xwid_switch: value.o3_xwid_switch,
            qr_sig: Vec::new(),
            key_exchange_ticket: Vec::new(),
            key_exchange_key: Vec::new(),
            login_cookie: Vec::new(),
        })
    }
}

pub(crate) fn encode_keystore_file(session: &Session) -> Vec<u8> {
    use prost::Message as _;
    wire::KeystoreFile {
        schema_version: 1,
        session: Some(session.into()),
    }
    .encode_to_vec()
}
pub(crate) fn decode_keystore_file(bytes: &[u8]) -> Result<Session> {
    use prost::Message as _;
    let file = wire::KeystoreFile::decode(bytes).context("Invalid protobuf keystore")?;
    anyhow::ensure!(
        file.schema_version == 1,
        "Unsupported keystore version: {}",
        file.schema_version
    );
    file.session
        .context("Keystore is missing its session")?
        .try_into()
}
