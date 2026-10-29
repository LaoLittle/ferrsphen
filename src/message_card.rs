use base64::{Engine as _, engine::general_purpose};
use rust_i18n::t;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    Link,
    Announcement,
    MiniApp,
    Music,
    Video,
    Location,
    Contact,
    Other,
}
impl CardKind {
    pub fn label(self) -> String {
        let key = match self {
            Self::Link => "card.link",
            Self::Announcement => "card.announcement",
            Self::MiniApp => "card.miniapp",
            Self::Music => "card.music",
            Self::Video => "common.video",
            Self::Location => "card.location",
            Self::Contact => "card.contact",
            Self::Other => "common.card",
        };
        t!(key).into_owned()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageCard {
    pub kind: CardKind,
    pub title: String,
    pub description: String,
    pub source: String,
    pub thumbnail: Option<String>,
    pub url: Option<String>,
}
fn text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    })
}
pub fn web_url(value: &str) -> Option<String> {
    let value = if value.starts_with("//") {
        format!("https:{value}")
    } else {
        value.to_owned()
    };
    let url = reqwest::Url::parse(&value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}
fn web_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| text(value, &[*key]).and_then(|value| web_url(&value)))
}
fn encoded_text(value: &Value, key: &str, encoded: bool) -> Option<String> {
    let text = text(value, &[key])?;
    if !encoded {
        return Some(text);
    }
    [
        general_purpose::STANDARD,
        general_purpose::STANDARD_NO_PAD,
        general_purpose::URL_SAFE,
        general_purpose::URL_SAFE_NO_PAD,
    ]
    .iter()
    .find_map(|engine| {
        engine
            .decode(&text)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .filter(|text| !text.trim().is_empty())
    })
}
impl MessageCard {
    pub fn parse(json: &str) -> Option<Self> {
        let root: Value = serde_json::from_str(json).ok()?;
        let app = root.get("app")?.as_str()?;
        let view = root.get("view").and_then(Value::as_str).unwrap_or_default();
        let meta = root.get("meta").and_then(Value::as_object);
        let detail = meta
            .and_then(|meta| {
                meta.get(view)
                    .filter(|value| value.is_object())
                    .or_else(|| meta.get("detail_1").filter(|value| value.is_object()))
                    .or_else(|| meta.values().find(|value| value.is_object()))
            })
            .unwrap_or(&Value::Null);
        let kind = if app == "com.tencent.mannounce" {
            CardKind::Announcement
        } else if app == "com.tencent.miniapp"
            || app.starts_with("com.tencent.miniapp.")
            || app.starts_with("com.tencent.miniapp_")
        {
            CardKind::MiniApp
        } else if view == "music" {
            CardKind::Music
        } else if view == "location" || view == "map" {
            CardKind::Location
        } else if view == "contact" {
            CardKind::Contact
        } else if view == "video" {
            CardKind::Video
        } else if view == "news"
            || text(detail, &["jumpUrl", "jump_url", "qqdocurl", "url"]).is_some()
        {
            CardKind::Link
        } else {
            CardKind::Other
        };
        let mut title = text(detail, &["title", "name"]);
        let mut description = text(
            detail,
            &["desc", "text", "summary", "content", "brief", "address"],
        )
        .unwrap_or_default();
        if kind == CardKind::Announcement {
            let encoded = detail
                .get("encode")
                .is_some_and(|value| value.as_u64() == Some(1) || value.as_str() == Some("1"));
            title = encoded_text(detail, "title", encoded);
            description = encoded_text(detail, "text", encoded).unwrap_or_default();
        }
        let title = title
            .or_else(|| text(&root, &["prompt", "title", "desc"]))
            .unwrap_or_else(|| kind.label());
        if description == title {
            description.clear();
        }
        let source =
            text(detail, &["tag", "source", "appName", "appname"]).unwrap_or_else(|| kind.label());
        let thumbnail = web_field(detail, &["preview", "cover", "image", "pic", "icon"]);
        let url = web_field(
            detail,
            &["jumpUrl", "jump_url", "qqdocurl", "url", "contentJumpUrl"],
        )
        .or_else(|| web_field(&root, &["jumpUrl", "url"]));
        Some(Self {
            kind,
            title,
            description,
            source,
            thumbnail,
            url,
        })
    }
    pub fn thumbnail_key(&self) -> Option<String> {
        self.thumbnail
            .as_ref()
            .map(|url| format!("card-preview:{}", sha1_smol::Sha1::from(url).digest()))
    }
    pub fn summary(&self) -> String {
        if self.title.starts_with('[') {
            self.title.clone()
        } else {
            format!("[{}] {}", self.kind.label(), self.title)
        }
    }
    pub fn detail_text(&self) -> String {
        [&self.title, &self.description]
            .into_iter()
            .filter(|text| !text.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn announcement_decodes_declared_base64_and_preserves_plain_text() {
        let encoded = serde_json::json!({"app":"com.tencent.mannounce","view":"main","meta":{"mannounce":{"encode":1,"title":"576k5YWs5ZGK","text":"5rWL6K+V"}}}).to_string();
        let card = MessageCard::parse(&encoded).unwrap();
        assert_eq!(
            (card.kind, card.title.as_str(), card.description.as_str()),
            (CardKind::Announcement, "群公告", "测试")
        );
        let plain = serde_json::json!({"app":"com.tencent.mannounce","meta":{"mannounce":{"encode":0,"title":"Title","text":"1234"}}}).to_string();
        assert_eq!(MessageCard::parse(&plain).unwrap().description, "1234");
    }
    #[test]
    fn invite_keeps_preview_and_content_without_launching_a_qq_deep_link() {
        let json = serde_json::json!({"app":"com.tencent.tuwen.lua","view":"news","meta":{"news":{"title":"Group invitation","desc":"Join this group","preview":"https://p.qlogo.cn/gh/42/42/100","tag":"Invite","jumpUrl":"mqqapi://group/invite_join?groupcode=42"}}}).to_string();
        let card = MessageCard::parse(&json).unwrap();
        assert_eq!(card.kind, CardKind::Link);
        assert_eq!(card.description, "Join this group");
        assert!(card.thumbnail.is_some());
        assert!(card.url.is_none());
        assert!(card.thumbnail_key().unwrap().starts_with("card-preview:"));
    }
    #[test]
    fn shared_cards_select_the_active_view_and_keep_supported_links() {
        for (app, view, expected) in [
            ("com.tencent.structmsg", "music", CardKind::Music),
            (
                "com.tencent.miniapp_01",
                "view_8C8E89B49BE609866298ADDFF2DBABA4",
                CardKind::MiniApp,
            ),
            ("com.tencent.miniapp", "detail_1", CardKind::MiniApp),
            ("test", "video", CardKind::Video),
            ("test", "location", CardKind::Location),
            ("test", "contact", CardKind::Contact),
        ] {
            let json = serde_json::json!({"app":app,"view":view,"meta":{"unrelated":{"title":"Wrong"},(view):{"title":"Title","desc":"Description","preview":"//example.com/cover.png","jumpUrl":"https://example.com/detail"}}}).to_string();
            let card = MessageCard::parse(&json).unwrap();
            assert_eq!(card.kind, expected);
            assert_eq!(card.title, "Title");
            assert_eq!(card.description, "Description");
            assert_eq!(
                card.thumbnail.as_deref(),
                Some("https://example.com/cover.png")
            );
            assert_eq!(card.url.as_deref(), Some("https://example.com/detail"));
        }
    }
    #[test]
    fn web_links_fall_back_from_app_deep_links_to_available_web_urls() {
        let json = serde_json::json!({"app":"com.tencent.miniapp_01","meta":{"detail_1":{"title":"Mini app","jumpUrl":"mqqapi://miniapp/open","qqdocurl":"https://example.com/app","preview":"not an image URL","cover":"https://example.com/cover.png"}}}).to_string();
        let card = MessageCard::parse(&json).unwrap();
        assert_eq!(card.url.as_deref(), Some("https://example.com/app"));
        assert_eq!(
            card.thumbnail.as_deref(),
            Some("https://example.com/cover.png")
        );
    }
    #[test]
    fn invalid_and_unknown_cards_preserve_a_readable_fallback() {
        assert!(MessageCard::parse("{}").is_none());
        assert!(MessageCard::parse("not json").is_none());
        let card = MessageCard::parse(
            r#"{"app":"unknown","prompt":"Readable prompt","meta":{"x":{"opaque":"retained"}}}"#,
        )
        .unwrap();
        assert_eq!(card.title, "Readable prompt");
        assert_eq!(card.kind, CardKind::Other);
        for url in [
            "javascript:alert(1)",
            "file:///tmp/image",
            "data:image/png;base64,abcd",
            "https://user:password@example.com",
        ] {
            assert!(web_url(url).is_none());
        }
    }
}
