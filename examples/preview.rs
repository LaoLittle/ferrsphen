use atri_qq::{
    UserInfo,
    contacts::{Friend, Gender, Group, GroupMember, GroupMemberPermission},
    messages::{Conversation, Message, MessageElement},
};
use ferrsphen::backend::{Command, Update};
use ferrsphen::view;
use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};
fn main() {
    let app = ferrsphen::platform::application().with_assets(gpui_kit::assets::AllAssets);
    app.run(|cx| {
        gpui_kit::init(cx);
        ferrsphen::window_chrome::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        cx.set_window_appearance(Some(WindowAppearance::Dark));
        Theme::update(cx, |theme| {
            theme.colors.background = rgb(0x202020).into();
            theme.colors.muted = rgb(0x303030).into();
            theme.colors.border = rgb(0x343434).into();
        });
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        let (commands, mut command_receiver) = tokio::sync::mpsc::channel(128);
        sender
            .try_send(Update::Online(UserInfo {
                uin: 1,
                uid: "self".into(),
                nickname: "ferrsphen".into(),
                age: 0,
                gender: 0,
            }))
            .unwrap();
        sender
            .try_send(Update::Contacts {
                friends: vec![Friend {
                    uin: 2,
                    uid: "peer".into(),
                    nickname: "Lin".into(),
                    remark: String::new(),
                    personal_sign: String::new(),
                    qid: String::new(),
                    age: 0,
                    gender: Gender::Unknown(0),
                    category_id: 0,
                    category: None,
                }],
                groups: vec![Group {
                    uin: 42,
                    name: "群聊".into(),
                    member_count: 3,
                    max_member_count: 500,
                    created_time: 0,
                    description: None,
                    question: None,
                    announcement: Some("公告".into()),
                    remark: None,
                    owner_uid: None,
                    last_speak_time: 0,
                    latest_sequence: 3,
                }],
            })
            .unwrap();
        let members = (1..=3)
            .map(|uin| GroupMember {
                group_uin: 42,
                uin,
                uid: format!("member-{uin}"),
                nickname: format!("成员 {uin}"),
                permission: GroupMemberPermission::Member,
                level: 0,
                card: Some(format!("群昵称 {uin}")),
                special_title: None,
                join_time: 0,
                last_message_time: 0,
                mute_until: 0,
            })
            .collect();
        sender
            .try_send(Update::Members { group: 42, members })
            .unwrap();
        for (sequence, text) in [(1, "你好"), (2, "收到"), (3, "图片、文件和语音")] {
            sender
                .try_send(Update::Message(Message {
                    conversation: Conversation::Group { uin: 42 },
                    sender_uin: if sequence == 2 { 1 } else { 3 },
                    sender_uid: format!("member-{}", if sequence == 2 { 1 } else { 3 }),
                    recipient_uin: 42,
                    recipient_uid: String::new(),
                    sender_name: "成员".into(),
                    sequence,
                    client_sequence: sequence,
                    message_id: sequence,
                    random: 0,
                    time: 1_700_000_000 + sequence as i64 * 120,
                    is_self: sequence == 2,
                    is_bot: None,
                    elements: vec![MessageElement::Text(text.into())],
                    metadata: vec![],
                    raw: vec![],
                }))
                .unwrap();
        }
        cx.spawn(async move |_| {
            while let Some(command) = command_receiver.recv().await {
                if let Command::History { conversation, .. } = command {
                    let _ = sender
                        .send(Update::History {
                            key: ferrsphen::model::conversation_key(&conversation),
                            result: Ok((Vec::new(), None)),
                        })
                        .await;
                }
            }
        })
        .detach();
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        gpui_kit::open_window(
            ferrsphen::window_chrome::app_options(
                ferrsphen::window_chrome::AppWindowKind::Login,
                cx,
            ),
            cx,
            |window, cx| cx.new(|cx| view::AppView::new(commands, receiver, window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
