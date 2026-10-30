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
            ios_msf_imei: value.ios_msf_imei.clone(),
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
            nt_ticket_times: value.nt_ticket_times.map(Into::into),
            wt_ticket_times: value.wt_ticket_times.map(Into::into),
            ticket_generated_unix_seconds: value.ticket_generated_unix_seconds,
            ios_virtual_state: value.ios_virtual_state.as_ref().map(|state| {
                super::encode(state).expect("iOS virtual state maps to a storage record")
            }),
            ios_server_lists: if value.ios_server_lists.is_empty() {
                Vec::new()
            } else {
                super::encode(&value.ios_server_lists)
                    .expect("iOS server lists map to a storage record")
            },
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
            ios_msf_imei: value.ios_msf_imei,
            ios_virtual_state: value
                .ios_virtual_state
                .map(|bytes| super::decode(&bytes))
                .transpose()
                .context("Invalid iOS virtual state")?,
            ios_server_lists: if value.ios_server_lists.is_empty() {
                Default::default()
            } else {
                super::decode(&value.ios_server_lists).context("Invalid iOS server lists")?
            },
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
            nt_ticket_times: value.nt_ticket_times.map(Into::into),
            wt_ticket_times: value.wt_ticket_times.map(Into::into),
            ticket_generated_unix_seconds: value.ticket_generated_unix_seconds,
            qr_sig: Vec::new(),
            key_exchange_session: None,
            login_cookie: Vec::new(),
        })
    }
}

impl From<atri_qq::NtTicketTimeValues> for wire::NtTicketTimeValues {
    fn from(value: atri_qq::NtTicketTimeValues) -> Self {
        Self {
            raw_field1: value.raw_field1,
            raw_field2: value.raw_field2,
        }
    }
}
impl From<wire::NtTicketTimeValues> for atri_qq::NtTicketTimeValues {
    fn from(value: wire::NtTicketTimeValues) -> Self {
        Self {
            raw_field1: value.raw_field1,
            raw_field2: value.raw_field2,
        }
    }
}
impl From<atri_qq::NtTicketTimeMetadata> for wire::NtTicketTimeMetadata {
    fn from(value: atri_qq::NtTicketTimeMetadata) -> Self {
        Self {
            field12: value.field12.map(Into::into),
            field16: value.field16.map(Into::into),
            received_unix_ms: value.received_unix_ms,
        }
    }
}
impl From<wire::NtTicketTimeMetadata> for atri_qq::NtTicketTimeMetadata {
    fn from(value: wire::NtTicketTimeMetadata) -> Self {
        Self {
            field12: value.field12.map(Into::into),
            field16: value.field16.map(Into::into),
            received_unix_ms: value.received_unix_ms,
        }
    }
}
impl From<atri_qq::WtTicketTimeMetadata> for wire::WtTicketTimeMetadata {
    fn from(value: atri_qq::WtTicketTimeMetadata) -> Self {
        Self {
            next_refresh_unix_seconds: value.next_refresh_unix_seconds,
            expire_unix_seconds: value.expire_unix_seconds,
        }
    }
}
impl From<wire::WtTicketTimeMetadata> for atri_qq::WtTicketTimeMetadata {
    fn from(value: wire::WtTicketTimeMetadata) -> Self {
        Self {
            next_refresh_unix_seconds: value.next_refresh_unix_seconds,
            expire_unix_seconds: value.expire_unix_seconds,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_refresh_metadata_survives_keystore_and_database_encoding() {
        let session = Session {
            keystore: Keystore {
                nt_ticket_times: Some(atri_qq::NtTicketTimeMetadata {
                    field12: Some(atri_qq::NtTicketTimeValues {
                        raw_field1: Some(0),
                        raw_field2: None,
                    }),
                    field16: Some(atri_qq::NtTicketTimeValues {
                        raw_field1: Some(1_800_000_100),
                        raw_field2: Some(1_800_000_200),
                    }),
                    received_unix_ms: 1_800_000_000_123,
                }),
                wt_ticket_times: Some(atri_qq::WtTicketTimeMetadata {
                    next_refresh_unix_seconds: 1_800_000_300,
                    expire_unix_seconds: 1_800_000_400,
                }),
                ticket_generated_unix_seconds: Some(1_800_000_000),
                ..Default::default()
            },
            machine_id: "device".into(),
            qua: "test".into(),
        };
        let restored = decode_keystore_file(&encode_keystore_file(&session)).unwrap();
        assert!(restored.keystore == session.keystore);
        let encoded = super::super::encode_session(&session).unwrap();
        let restored: Session = super::super::decode(&encoded).unwrap();
        assert!(restored.keystore == session.keystore);
    }

    #[test]
    fn ios_identity_survives_both_protobuf_formats_but_exchange_sessions_never_do() {
        let ticket = b"synthetic-private-nt-exchange-ticket";
        let session = Session {
            keystore: Keystore {
                ios_msf_imei: "A0000000-0000-4000-8000-000000000001".into(),
                a1: vec![1, 2, 3],
                qr_sig: vec![4],
                login_cookie: vec![5],
                key_exchange_session: Some(
                    atri_qq::NtKeyExchangeSession::new(
                        atri_qq::ClientPlatform::IPhone,
                        bytes::Bytes::from_static(&[7; 16]),
                        bytes::Bytes::from_static(ticket),
                        600,
                    )
                    .unwrap(),
                ),
                ios_virtual_state: Some(
                    serde_json::from_value(serde_json::json!({
                        "hardware_model": "iPhone13,4",
                        "collector_uuid": "00000000-0000-4000-8000-000000000001",
                        "vendor_uuid": "00000000-0000-4000-8000-000000000002",
                        "home_uuid": "00000000-0000-4000-8000-000000000003",
                        "bundle_uuid": "00000000-0000-4000-8000-000000000004",
                        "runtime_id": "runtime",
                        "installed_unix_micros": 1,
                        "boot_unix_micros": 2,
                        "filesystem_created_unix_micros": 3,
                        "private_var_inode": 4,
                        "mobile_preferences_inode": 5,
                        "xwid_candidate": "xwid",
                        "connection_sequence": 6,
                        "guild_sync": {"7": {"last_msg_time": 8, "last_pull_time": null, "entries": {"9": 10}}},
                        "registered_accounts": [11],
                        "token": {"expires_at": 12, "token": [13, 14], "associated_uin": "11"}
                    }))
                    .unwrap(),
                ),
                ios_server_lists: serde_json::from_value(serde_json::json!({
                    "nossid_wifi": {"server_time": 1, "carrier": "carrier"}
                }))
                .unwrap(),
                ..Default::default()
            },
            machine_id: "device".into(),
            qua: atri_qq::Profile::iphone().qua,
        };
        let file = encode_keystore_file(&session);
        let record = super::super::encode_session(&session).unwrap();
        for bytes in [&file, &record] {
            assert!(!bytes.windows(ticket.len()).any(|window| window == ticket));
        }
        let from_file = decode_keystore_file(&file).unwrap();
        let from_record: Session = super::super::decode(&record).unwrap();
        for restored in [from_file, from_record] {
            assert_eq!(
                restored.keystore.ios_msf_imei,
                session.keystore.ios_msf_imei
            );
            assert_eq!(restored.keystore.a1, session.keystore.a1);
            assert!(restored.keystore.ios_virtual_state == session.keystore.ios_virtual_state);
            assert_eq!(
                serde_json::to_value(&restored.keystore.ios_server_lists).unwrap(),
                serde_json::to_value(&session.keystore.ios_server_lists).unwrap()
            );
            assert_eq!(restored.qua, session.qua);
            assert!(restored.keystore.key_exchange_session.is_none());
            assert!(restored.keystore.qr_sig.is_empty());
            assert!(restored.keystore.login_cookie.is_empty());
        }
    }
    #[test]
    fn legacy_cbor_exchange_fields_are_discarded_while_ios_identity_is_preserved() {
        let legacy = serde_json::json!({"keystore":{"uin":123,"ios_msf_imei":"device-imei","a1":[1,2],"key_exchange_key":[7,7],"key_exchange_ticket":[8,8]},"machine_id":"device","qua":"legacy-profile"});
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&legacy, &mut bytes).unwrap();
        let session: Session = super::super::decode(&bytes).unwrap();
        assert_eq!(session.keystore.ios_msf_imei, "device-imei");
        assert!(session.keystore.key_exchange_session.is_none());
        let restored = decode_keystore_file(&encode_keystore_file(&session)).unwrap();
        assert_eq!(restored.keystore.ios_msf_imei, "device-imei");
        assert_eq!(restored.keystore.a1, [1, 2]);
    }

    #[test]
    fn older_protobuf_keystores_keep_ticket_times_absent() {
        let store: Keystore = wire::Keystore::default().try_into().unwrap();
        assert!(store.ios_msf_imei.is_empty());
        assert!(store.key_exchange_session.is_none());
        assert_eq!(store.nt_ticket_times, None);
        assert_eq!(store.wt_ticket_times, None);
        assert_eq!(store.ticket_generated_unix_seconds, None);
        assert_eq!(store.next_ticket_refresh_unix_seconds(), None);
    }
}
