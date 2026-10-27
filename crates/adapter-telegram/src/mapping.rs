use chat_adapter::*;
use grammers_client::{
    media::Media as TgMedia,
    message::Message as TgMessage,
    peer::{Dialog, Peer},
};
use grammers_tl_types::{self as tl, Serializable};
use serde_json::json;

pub fn user(user: &grammers_client::peer::User) -> User {
    User {
        id: user.id().bot_api_dialog_id_unchecked().to_string(),
        name: user.full_name(),
        avatar: avatar(user.id().bot_api_dialog_id_unchecked().to_string())
            .filter(|_| user.photo().is_some()),
    }
}
fn avatar(chat: String) -> Option<Box<Media>> {
    Some(Box::new(Media {
        id: format!("avatar:{chat}"),
        mime: Some("image/jpeg".into()),
        name: None,
        width: None,
        height: None,
        duration_ms: None,
        byte_length: None,
        data: json!({"chat":chat,"avatar":true}),
    }))
}
fn peer_avatar(peer: &Peer) -> Option<Box<Media>> {
    let has_photo = match peer {
        Peer::User(peer) => peer.photo().is_some(),
        Peer::Group(peer) => peer.photo().is_some(),
        Peer::Channel(peer) => peer.photo().is_some(),
        Peer::Community(peer) => peer.photo().is_some(),
    };
    avatar(peer.id().bot_api_dialog_id_unchecked().to_string()).filter(|_| has_photo)
}
pub fn conversation(account: &AccountId, dialog: &Dialog) -> Conversation {
    let id = dialog.peer_id().bot_api_dialog_id_unchecked().to_string();
    let kind = match dialog.peer() {
        Peer::User(_) => ChatKind::Direct,
        Peer::Group(_) => ChatKind::Group,
        Peer::Channel(_) => ChatKind::Channel,
        Peer::Community(_) => ChatKind::Group,
    };
    let unread = match &dialog.raw {
        tl::enums::Dialog::Dialog(dialog) => dialog.unread_count.max(0) as u32,
        _ => 0,
    };
    Conversation {
        id: account.chat(&id),
        kind,
        title: dialog.peer().name().unwrap_or("Telegram").into(),
        avatar: peer_avatar(dialog.peer()),
        unread,
        last_activity_ms: dialog
            .last_message
            .as_ref()
            .map_or(0, |message| message.date().as_second() * 1000),
    }
}
fn extension(kind: &str, layout: Layout, text: String, raw: Vec<u8>) -> Content {
    Content::Extension(ExtensionContent {
        kind: kind.into(),
        version: 1,
        data: json!({"tl":hex::encode(raw)}),
        layout,
        display: vec![DisplayPart::Text(text)],
    })
}
pub fn message(account: &AccountId, message: &TgMessage) -> Message {
    let chat = account.chat(message.peer_id().bot_api_dialog_id_unchecked().to_string());
    let sender = message
        .sender()
        .map(|peer| User {
            id: peer.id().bot_api_dialog_id_unchecked().to_string(),
            name: peer.name().unwrap_or("Telegram").into(),
            avatar: peer_avatar(peer),
        })
        .unwrap_or(User {
            id: message.sender_id().map_or_else(String::new, |id| {
                id.bot_api_dialog_id_unchecked().to_string()
            }),
            name: "Telegram".into(),
            avatar: None,
        });
    let mut content = Vec::new();
    if !message.text().is_empty() {
        content.push(Content::Text(message.text().into()));
    }
    if let tl::enums::Message::Service(service) = &message.raw {
        let text = match &service.action {
            tl::enums::MessageAction::ChatCreate(action) => format!("Created {}", action.title),
            tl::enums::MessageAction::ChatEditTitle(action) => {
                format!("Renamed the group to {}", action.title)
            }
            tl::enums::MessageAction::ChatAddUser(_) => "Added group members".into(),
            tl::enums::MessageAction::ChatDeleteUser(_) => "Removed a group member".into(),
            tl::enums::MessageAction::ChatJoinedByLink(_)
            | tl::enums::MessageAction::ChatJoinedByRequest => "Joined the group".into(),
            tl::enums::MessageAction::PinMessage => "Pinned a message".into(),
            tl::enums::MessageAction::HistoryClear => "Cleared the history".into(),
            tl::enums::MessageAction::ContactSignUp => "Joined Telegram".into(),
            _ => "Telegram event".into(),
        };
        content.push(extension(
            "telegram.service",
            Layout::Notice,
            text,
            service.action.to_bytes(),
        ));
    }
    if let Some(media) = message.media() {
        let mut reference = Media {
            id: format!("message:{}:{}", chat.id, message.id()),
            mime: None,
            name: None,
            width: None,
            height: None,
            duration_ms: None,
            byte_length: None,
            data: json!({"chat":chat.id,"message":message.id()}),
        };
        let part = match media {
            TgMedia::Photo(photo) => {
                reference.mime = Some("image/jpeg".into());
                reference.byte_length = photo.size().map(|size| size as u64);
                Content::Image(reference)
            }
            TgMedia::Document(document) => {
                reference.mime = document.mime_type().map(String::from);
                reference.name = document.name().map(String::from);
                reference.byte_length = document.size().map(|size| size as u64);
                reference.duration_ms = document
                    .duration()
                    .map(|duration| (duration * 1000.) as u64);
                if let Some((width, height)) = document.resolution() {
                    reference.width = u32::try_from(width).ok();
                    reference.height = u32::try_from(height).ok();
                }
                let voice = matches!(&document.raw.document,Some(tl::enums::Document::Document(doc)) if doc.attributes.iter().any(|attr|matches!(attr,tl::enums::DocumentAttribute::Audio(audio) if audio.voice)));
                match reference.mime.as_deref() {
                    _ if voice => Content::Voice(reference),
                    Some(mime) if mime.starts_with("video/") => Content::Video(reference),
                    Some(mime) if mime.starts_with("image/") => Content::Image(reference),
                    Some(mime) if mime.starts_with("audio/") => Content::Audio(reference),
                    _ => Content::File(reference),
                }
            }
            TgMedia::Sticker(sticker) => {
                reference.name = Some(sticker.raw_attrs.alt);
                reference.mime = sticker.document.mime_type().map(String::from);
                Content::Sticker(reference)
            }
            TgMedia::Contact(contact) => extension(
                "telegram.contact",
                Layout::Card,
                format!(
                    "{} {} · {}",
                    contact.raw.first_name, contact.raw.last_name, contact.raw.phone_number
                ),
                contact.raw.to_bytes(),
            ),
            TgMedia::Dice(dice) => extension(
                "telegram.dice",
                Layout::Bubble,
                format!("{} {}", dice.raw.emoticon, dice.raw.value),
                dice.raw.to_bytes(),
            ),
            TgMedia::Poll(poll) => extension(
                "telegram.poll",
                Layout::Card,
                "Poll".into(),
                poll.raw.to_bytes(),
            ),
            TgMedia::Geo(geo) => extension(
                "telegram.location",
                Layout::Card,
                format!("{}, {}", geo.raw.lat, geo.raw.long),
                geo.raw.to_bytes(),
            ),
            TgMedia::Venue(venue) => extension(
                "telegram.venue",
                Layout::Card,
                format!("{}\n{}", venue.raw_venue.title, venue.raw_venue.address),
                venue.raw_venue.to_bytes(),
            ),
            TgMedia::WebPage(page) => extension(
                "telegram.web_page",
                Layout::Card,
                "Link preview".into(),
                page.raw.to_bytes(),
            ),
            _ => extension(
                "telegram.media",
                Layout::Card,
                "Telegram media".into(),
                message.raw.to_bytes(),
            ),
        };
        content.push(part);
    }
    let reply = message.reply_to_message_id().map(|id| {
        Box::new(Reply {
            target: MessageId {
                chat: chat.clone(),
                id: id.to_string(),
            },
            sender: User {
                id: String::new(),
                name: "Telegram".into(),
                avatar: None,
            },
            preview: vec![],
        })
    });
    Message {
        id: MessageId {
            chat,
            id: message.id().to_string(),
        },
        client_id: None,
        sender,
        time_ms: message.date().as_second() * 1000,
        outgoing: message.outgoing(),
        content,
        reply,
        deleted: false,
    }
}
