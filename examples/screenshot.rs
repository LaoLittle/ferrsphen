use atri_qq::{
    UserInfo,
    contacts::{Friend, Gender, Group, GroupAnnouncement, GroupMember, GroupMemberPermission},
    media::{MediaConversation, MediaKind, MediaReference},
    messages::{Conversation, Message, MessageElement},
};
use ferrsphen::{backend::Update, model::Model, view::AppView};
use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};
use std::{path::Path, sync::Arc, time::Duration};

fn main() -> anyhow::Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "assets/screenshot.png".into());
    let temporary = tempfile::tempdir()?;
    let image_path = temporary.path().join("seaside.png");
    seaside(&image_path)?;
    let platform = gpui_kit::platform::current_platform(true);
    let mut cx = HeadlessAppContext::with_platform(
        platform.text_system(),
        Arc::new(gpui_kit::assets::AllAssets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        rust_i18n::set_locale("zh-CN");
        gpui_kit::init(cx);
        ferrsphen::window_chrome::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.set_window_appearance(Some(WindowAppearance::Dark));
        Theme::update(cx, |theme| {
            theme.colors.background = rgb(0x202020).into();
            theme.colors.muted = rgb(0x303030).into();
            theme.colors.border = rgb(0x343434).into();
            theme.colors.accent = rgb(0x2d3f52).into();
            theme.colors.primary = rgb(0x508be8).into();
        });
    });
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    let (commands, _command_receiver) = tokio::sync::mpsc::channel(128);
    cx.open_window(size(px(1240.), px(820.)), |window, cx| {
        cx.new(|cx| AppView::new(commands, receiver, window, cx))
    })?;
    for update in mock_updates(image_path) {
        sender.try_send(update)?;
    }
    for _ in 0..90 {
        cx.run_until_parked();
        cx.advance_clock(Duration::from_millis(16));
        let windows = cx.update(|cx| cx.windows());
        for window in windows {
            cx.update_window(window, |_, window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            })?;
        }
    }
    let handle = cx
        .update(|cx| cx.windows().into_iter().next())
        .expect("mock window");
    let screenshot = cx.capture_screenshot(handle)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    screenshot.save(&output)?;
    println!("{}", output.display());
    Ok(())
}

fn mock_updates(image_path: std::path::PathBuf) -> Vec<Update> {
    let mut model = Model::default();
    model.contacts(
        vec![friend(2, "阿岚"), friend(3, "Momo"), friend(4, "小夏")],
        vec![group(42, "周末去海边"), group(43, "随手拍")],
    );
    model.selected = Some("group:42".into());
    model.history.insert(
        "group:42".into(),
        ferrsphen::model::History {
            loaded: true,
            ..Default::default()
        },
    );
    let mut picture = MediaReference::new(MediaKind::Image, MediaConversation::Group { uin: 42 });
    picture.width = 320;
    picture.height = 180;
    for (sequence, sender, element) in [
        (4, 2, MessageElement::Image(picture)),
        (
            5,
            3,
            MessageElement::Text("这个海边好看！我带相机 📷".into()),
        ),
        (6, 1, MessageElement::Text("那就这么定了，周六见！".into())),
    ] {
        model.receive(
            message(Conversation::Group { uin: 42 }, sequence, sender, element),
            false,
        );
    }
    for (uin, text) in [(2, "路线发在群里了"), (3, "周六见！"), (4, "收到，谢谢")] {
        let mut row = message(
            Conversation::Friend {
                uin,
                uid: format!("user-{uin}"),
            },
            1,
            uin,
            MessageElement::Text(text.into()),
        );
        row.time -= 3600 + uin as i64 * 300;
        model.receive(row, false);
    }
    let mut row = message(
        Conversation::Group { uin: 43 },
        1,
        2,
        MessageElement::Text("今天的晚霞很好看".into()),
    );
    row.time -= 1800;
    model.receive(row, false);
    model.members.insert(
        42,
        [(1, "Lin"), (2, "阿岚"), (3, "Momo")]
            .into_iter()
            .map(|(uin, name)| GroupMember {
                group_uin: 42,
                uin,
                uid: format!("user-{uin}"),
                nickname: name.into(),
                permission: if uin == 1 {
                    GroupMemberPermission::Owner
                } else {
                    GroupMemberPermission::Member
                },
                level: 0,
                card: Some(name.into()),
                special_title: None,
                join_time: 0,
                last_message_time: 0,
                mute_until: 0,
            })
            .collect(),
    );
    model.announcements.insert(
        42,
        Ok(vec![GroupAnnouncement {
            id: "weekend".into(),
            sender_uin: 1,
            published_at: 1_792_780_200,
            title: "周末小计划".into(),
            text: "周六 09:00 · 地铁站 A 口集合\n记得带饮用水和防晒用品。".into(),
            pictures: vec![],
            pinned: true,
        }]),
    );
    for chat in &mut model.chats {
        chat.unread = if chat.conversation == (Conversation::Group { uin: 43 }) {
            2
        } else {
            0
        };
    }
    vec![
        Update::Online(UserInfo {
            uin: 1,
            uid: "user-1".into(),
            nickname: "Lin".into(),
            age: 0,
            gender: 0,
        }),
        Update::Asset {
            key: "preview:group:42:4:4:0".into(),
            path: image_path,
        },
        Update::Restored(Box::new(model)),
    ]
}

fn friend(uin: u64, name: &str) -> Friend {
    Friend {
        uin,
        uid: format!("user-{uin}"),
        nickname: name.into(),
        remark: String::new(),
        personal_sign: String::new(),
        qid: String::new(),
        age: 0,
        gender: Gender::Unknown(0),
        category_id: 0,
        category: None,
    }
}
fn group(uin: u64, name: &str) -> Group {
    Group {
        uin,
        name: name.into(),
        member_count: 3,
        max_member_count: 500,
        created_time: 0,
        description: None,
        question: None,
        announcement: None,
        remark: None,
        owner_uid: None,
        last_speak_time: 0,
        latest_sequence: 6,
    }
}
fn message(
    conversation: Conversation,
    sequence: u64,
    sender: u64,
    element: MessageElement,
) -> Message {
    Message {
        conversation,
        sender_uin: sender,
        sender_uid: format!("user-{sender}"),
        recipient_uin: 42,
        recipient_uid: String::new(),
        sender_name: String::new(),
        sequence,
        client_sequence: sequence,
        message_id: sequence,
        random: 0,
        time: 1_792_780_200 + sequence as i64 * 60,
        is_self: sender == 1,
        is_bot: None,
        elements: vec![element],
        metadata: vec![],
        raw: vec![],
    }
}
fn seaside(path: &Path) -> image::ImageResult<()> {
    let image = image::RgbImage::from_fn(960, 540, |x, y| {
        let horizon = 220.;
        let xf = x as f32;
        let yf = y as f32;
        let color = if (xf - 735.).powi(2) + (yf - 105.).powi(2) < 37_f32.powi(2) {
            [255, 232, 177]
        } else if x < 450
            && yf >= horizon - (xf / 450. * std::f32::consts::PI).sin().powi(2) * 76.
            && yf < horizon
        {
            [91, 134, 146]
        } else if yf < horizon {
            let t = yf / horizon;
            [
                (112. + t * 80.) as u8,
                (167. + t * 46.) as u8,
                (211. + t * 24.) as u8,
            ]
        } else if yf < 418. + (xf / 220.).sin() * 24. {
            let wave = ((yf + (xf / 65.).sin() * 5.) / 17.).sin();
            let t = ((yf - horizon) / 220.).clamp(0., 1.);
            [
                (61. + t * 59. + wave * 5.) as u8,
                (143. + t * 43. + wave * 8.) as u8,
                (167. + t * 17. + wave * 7.) as u8,
            ]
        } else {
            let grain = ((x * 17 + y * 31) % 13) as u8;
            [222 + grain, 204 + grain, 168 + grain]
        };
        image::Rgb(color)
    });
    image.save(path)
}
