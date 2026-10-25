#[cfg(test)]
use crate::proto::expressions::FavoriteList;
use crate::proto::expressions::{
    CommonFace, Environment, FaceElement, FaceWire, FavoriteRequest, FavoriteResponse, ModernFace,
};
use anyhow::{Result, bail, ensure};
use atri_qq::{
    Encryption, RequestType, ServiceClient,
    messages::{MessageElement, OpaqueElement, OpaqueScope},
};
use prost::Message as _;
use serde::{Deserialize, Serialize};
use std::{sync::LazyLock, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Face {
    pub id: u32,
    pub name: String,
}
pub const FACES: &[(u32, &str)] = &[
    (14, "微笑"),
    (1, "撇嘴"),
    (2, "色"),
    (3, "发呆"),
    (4, "得意"),
    (5, "流泪"),
    (6, "害羞"),
    (7, "闭嘴"),
    (8, "睡"),
    (9, "大哭"),
    (10, "尴尬"),
    (11, "发怒"),
    (12, "调皮"),
    (13, "呲牙"),
    (0, "惊讶"),
    (15, "难过"),
    (16, "酷"),
    (18, "抓狂"),
    (19, "吐"),
    (20, "偷笑"),
    (21, "可爱"),
    (22, "白眼"),
    (23, "傲慢"),
    (25, "困"),
    (26, "惊恐"),
    (27, "流汗"),
    (28, "憨笑"),
    (29, "悠闲"),
    (30, "奋斗"),
    (32, "疑问"),
    (33, "嘘"),
    (34, "晕"),
    (35, "折磨"),
    (36, "衰"),
    (49, "拥抱"),
    (53, "蛋糕"),
    (54, "闪电"),
    (55, "炸弹"),
    (60, "咖啡"),
    (66, "爱心"),
    (74, "太阳"),
    (75, "月亮"),
    (76, "赞"),
    (77, "踩"),
    (78, "握手"),
    (79, "胜利"),
    (85, "飞吻"),
    (89, "西瓜"),
];
#[derive(Deserialize)]
struct EmojiCatalog {
    version: u32,
    animated_base_url: String,
    faces: Vec<EmojiMapping>,
}
#[derive(Deserialize)]
struct EmojiMapping {
    qq: u32,
    unicode: String,
    emoji: String,
    animated: bool,
}
static NOTO: LazyLock<EmojiCatalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../assets/noto-emoji-map.json"))
        .expect("valid bundled emoji mapping")
});
impl Face {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            name: FACES
                .iter()
                .find(|(index, _)| *index == id)
                .map(|(_, name)| (*name).into())
                .unwrap_or_else(|| format!("表情{id}")),
        }
    }
    pub fn text(&self) -> String {
        format!("[{}]", self.name)
    }
    pub fn asset_key(&self) -> String {
        self.noto().map_or_else(
            || self.fallback_asset_key(),
            |mapping| {
                format!(
                    "face:noto:v{}:{}:{}",
                    NOTO.version, self.id, mapping.unicode
                )
            },
        )
    }
    pub fn poster_asset_key(&self) -> String {
        format!("{}:poster", self.asset_key())
    }
    pub fn poster_url(&self) -> Option<String> {
        self.noto()
            .map(|mapping| format!("{}/{}/emoji.svg", NOTO.animated_base_url, mapping.unicode))
    }
    pub fn fallback_asset_key(&self) -> String {
        format!("face:qq:{}", self.id)
    }
    pub fn display_text(&self) -> String {
        self.noto()
            .map_or_else(|| self.text(), |mapping| mapping.emoji.clone())
    }
    fn noto(&self) -> Option<&'static EmojiMapping> {
        NOTO.faces.iter().find(|mapping| mapping.qq == self.id)
    }
    pub fn image_url(&self) -> Option<String> {
        if let Some(mapping) = self.noto() {
            let filename = if mapping.animated {
                "lottie.json"
            } else {
                "emoji.svg"
            };
            return Some(format!(
                "{}/{}/{filename}",
                NOTO.animated_base_url, mapping.unicode
            ));
        }
        self.fallback_image_url()
    }
    pub fn fallback_image_url(&self) -> Option<String> {
        (self.id < 256).then(|| {
            format!(
                "https://qzonestyle.gtimg.cn/qzone/em/e{}.gif",
                match self.id {
                    14 => 100,
                    0 => 114,
                    id => id + 100,
                }
            )
        })
    }
    pub fn element(&self) -> MessageElement {
        let element = if self.id < 256 {
            FaceElement {
                face: Some(FaceWire {
                    id: Some(self.id),
                    old: Some((0x1441u16 + self.id as u16).to_be_bytes().to_vec()),
                    reserve: Some(vec![0, 1, 0, 4, 0x52, 0xcc, 0xf5, 0xd0]),
                }),
                common: vec![],
            }
        } else {
            FaceElement {
                face: None,
                common: vec![CommonFace {
                    service: Some(33),
                    business: Some(1),
                    payload: ModernFace {
                        id: Some(self.id),
                        name: Some(format!("/{}", self.name)),
                        compatibility: Some(format!("/{}", self.name)),
                    }
                    .encode_to_vec(),
                }],
            }
        };
        MessageElement::Opaque(OpaqueElement {
            scope: OpaqueScope::Element,
            field: if self.id < 256 { 2 } else { 53 },
            payload: element.encode_to_vec(),
        })
    }
    pub fn from_element(element: &MessageElement) -> Option<Self> {
        let MessageElement::Opaque(record) = element else {
            return None;
        };
        if record.scope != OpaqueScope::Element || !matches!(record.field, 2 | 53) {
            return None;
        }
        let element = FaceElement::decode(record.payload.as_slice()).ok()?;
        if let Some(face) = element.face {
            return face.id.map(Self::new);
        }
        for common in element.common {
            if common.service == Some(33) {
                let face = ModernFace::decode(common.payload.as_slice()).ok()?;
                let mut result = Self::new(face.id?);
                if let Some(name) = face.name.filter(|name| !name.trim_matches('/').is_empty()) {
                    result.name = name.trim_start_matches('/').to_owned();
                }
                return Some(result);
            }
        }
        None
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Favorite {
    pub id: String,
    pub url: String,
}
pub async fn fetch_favorites(client: &dyn ServiceClient) -> Result<Vec<Favorite>> {
    let state = client.snapshot().await;
    let uin = state.keystore.uin;
    ensure!(uin != 0, "账号未登录");
    let request = FavoriteRequest {
        environment: Some(Environment {
            platform: Some(1),
            os: Some(state.profile.kernel),
            version: Some(state.profile.version),
        }),
        uin: Some(uin),
        operation: Some(1),
        sync: Some(1),
    };
    let bytes = client
        .request(
            "Faceroam.OpReq",
            request.encode_to_vec().into(),
            RequestType::D2,
            Encryption::D2,
            Duration::from_secs(30),
        )
        .await?;
    let response = FavoriteResponse::decode(bytes)?;
    if response.code.unwrap_or(0) != 0 {
        bail!(
            "收藏表情加载失败：{} ({})",
            response.message.unwrap_or_default(),
            response.code.unwrap_or_default()
        );
    }
    let Some(list) = response.list else {
        return Ok(vec![]);
    };
    if list.ids.is_empty() {
        return Ok(vec![]);
    }
    let category = list
        .category
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("收藏表情缺少下载分类"))?;
    let mut seen = std::collections::HashSet::new();
    list.ids
        .into_iter()
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .map(|id| {
            let mut url = reqwest::Url::parse("https://p.qpic.cn/")?;
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("表情地址无效"))?
                .pop_if_empty()
                .push(&category)
                .push(&uin.to_string())
                .push(&id)
                .push("0");
            Ok(Favorite {
                id,
                url: url.into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noto_mapping_covers_picker_without_changing_qq_wire_ids() {
        let ids = NOTO
            .faces
            .iter()
            .map(|mapping| mapping.qq)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), NOTO.faces.len());
        assert_eq!(ids, FACES.iter().map(|&(id, _)| id).collect());
        for mapping in &NOTO.faces {
            assert!(!mapping.emoji.is_empty());
            assert!(mapping.unicode.split('_').all(|part| {
                u32::from_str_radix(part, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .is_some()
            }));
            let face = Face::new(mapping.qq);
            assert!(
                face.image_url()
                    .unwrap()
                    .starts_with("https://fonts.gstatic.com/s/e/notoemoji/latest/")
            );
            assert!(face.asset_key().starts_with("face:noto:v2:"));
            assert_eq!(Face::from_element(&face.element()).unwrap().id, mapping.qq);
        }
        assert!(
            Face::new(32)
                .image_url()
                .unwrap()
                .ends_with("/1f914/lottie.json")
        );
        assert_eq!(Face::new(32).display_text(), "🤔");
        assert!(
            Face::new(53)
                .poster_url()
                .unwrap()
                .ends_with("/1f382/emoji.svg")
        );
        assert!(Face::new(999).image_url().is_none());
        assert_eq!(Face::new(999).display_text(), "[表情999]");
        assert!(
            Face::new(14)
                .image_url()
                .unwrap()
                .ends_with("/1f642/lottie.json")
        );
        assert!(
            Face::new(74)
                .image_url()
                .unwrap()
                .ends_with("/2600/emoji.svg")
        );
    }
    #[test]
    fn faces_use_native_wire_elements_and_survive_protocol_serialization() {
        let conversation = atri_qq::messages::Conversation::Group { uin: 42 };
        for id in [0, 14, 76, 300] {
            let face = Face::new(id);
            let element = face.element();
            let encoded = atri_qq::messages::encode_elements(&[element], &[]).unwrap();
            let decoded = atri_qq::messages::decode_elements(&encoded, &conversation, 100).unwrap();
            assert_eq!(decoded.elements.len(), 1);
            assert_eq!(Face::from_element(&decoded.elements[0]).unwrap().id, id);
        }
        let MessageElement::Opaque(wire) = Face::new(14).element() else {
            unreachable!()
        };
        assert_eq!(
            hex::encode(wire.payload),
            "1210080e1202144f5a080001000452ccf5d0"
        );
    }
    struct FavoriteClient {
        response: Vec<u8>,
        state: std::sync::Mutex<atri_qq::SessionState>,
        packets: tokio::sync::broadcast::Sender<atri_qq::IncomingPacket>,
    }
    impl FavoriteClient {
        fn new(response: FavoriteResponse) -> Self {
            let (packets, _) = tokio::sync::broadcast::channel(4);
            let keystore = atri_qq::Keystore {
                uin: 12345,
                ..Default::default()
            };
            Self {
                response: response.encode_to_vec(),
                state: std::sync::Mutex::new(atri_qq::SessionState {
                    profile: atri_qq::Profile::linux(),
                    keystore,
                    machine_id: String::new(),
                    online: true,
                }),
                packets,
            }
        }
    }
    #[async_trait::async_trait]
    impl ServiceClient for FavoriteClient {
        async fn request(
            &self,
            command: &str,
            body: bytes::Bytes,
            request_type: RequestType,
            encryption: Encryption,
            timeout: Duration,
        ) -> atri_qq::Result<bytes::Bytes> {
            assert_eq!(command, "Faceroam.OpReq");
            assert!(matches!(request_type, RequestType::D2));
            assert!(matches!(encryption, Encryption::D2));
            assert_eq!(timeout, Duration::from_secs(30));
            let request = FavoriteRequest::decode(body).unwrap();
            assert_eq!(request.uin, Some(12345));
            assert_eq!(request.operation, Some(1));
            assert_eq!(request.sync, Some(1));
            let environment = request.environment.unwrap();
            assert_eq!(environment.platform, Some(1));
            assert_eq!(
                environment.version.as_deref(),
                Some(atri_qq::Profile::linux().version.as_str())
            );
            Ok(self.response.clone().into())
        }
        async fn snapshot(&self) -> atri_qq::SessionState {
            self.state.lock().unwrap().clone()
        }
        async fn replace_state(&self, state: atri_qq::SessionState) {
            *self.state.lock().unwrap() = state;
        }
        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<atri_qq::IncomingPacket> {
            self.packets.subscribe()
        }
    }
    #[tokio::test]
    async fn favorites_request_preserves_order_and_builds_escaped_account_urls() {
        let client = FavoriteClient::new(FavoriteResponse {
            code: Some(0),
            message: None,
            list: Some(FavoriteList {
                ids: vec!["ABC".into(), "DEF/G".into(), "ABC".into(), "".into()],
                category: Some("vip/emoji".into()),
            }),
        });
        let items = fetch_favorites(&client).await.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].url, "https://p.qpic.cn/vip%2Femoji/12345/ABC/0");
        assert_eq!(items[1].id, "DEF/G");
        assert_eq!(
            items[1].url,
            "https://p.qpic.cn/vip%2Femoji/12345/DEF%2FG/0"
        );
    }
    #[tokio::test]
    async fn favorites_distinguish_empty_lists_server_errors_and_invalid_download_metadata() {
        for list in [
            None,
            Some(FavoriteList {
                ids: vec![],
                category: None,
            }),
        ] {
            let client = FavoriteClient::new(FavoriteResponse {
                code: Some(0),
                message: None,
                list,
            });
            assert!(fetch_favorites(&client).await.unwrap().is_empty());
        }
        let client = FavoriteClient::new(FavoriteResponse {
            code: Some(17),
            message: Some("server detail".into()),
            list: None,
        });
        let error = fetch_favorites(&client).await.unwrap_err().to_string();
        assert!(error.contains("17") && error.contains("server detail"));
        let client = FavoriteClient::new(FavoriteResponse {
            code: Some(0),
            message: None,
            list: Some(FavoriteList {
                ids: vec!["ABC".into()],
                category: None,
            }),
        });
        assert!(fetch_favorites(&client).await.is_err());
    }
    #[tokio::test]
    async fn face_and_favorite_records_round_trip_through_local_database() {
        let root = tempfile::tempdir().unwrap();
        let db = crate::storage::Database::open(root.path()).unwrap();
        let face = Face::new(14).element();
        db.put("face", &face).await.unwrap();
        assert_eq!(
            Face::from_element(&db.get::<MessageElement>("face").unwrap().unwrap())
                .unwrap()
                .id,
            14
        );
        let items = vec![Favorite {
            id: "ABC".into(),
            url: "https://p.qpic.cn/test/12345/ABC/0".into(),
        }];
        db.put("favorites/12345", &items).await.unwrap();
        assert_eq!(
            db.get::<Vec<Favorite>>("favorites/12345").unwrap().unwrap(),
            items
        );
        assert!(
            db.get::<Vec<Favorite>>("favorites/67890")
                .unwrap()
                .is_none()
        );
    }
}
