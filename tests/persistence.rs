use atri_qq::{
    contacts::{Friend, Gender, GroupMember, GroupMemberPermission},
    media::{MediaConversation, MediaKind, MediaReference},
    messages::{
        Conversation, ForwardReference, GroupFileMessage, Mention, Message, MessageElement,
        OpaqueElement, OpaqueScope, Reply,
    },
};
use ferrsphen::{
    model::{Model, conversation_key, display_name},
    storage::Database,
};
fn message() -> Message {
    let reference = MediaReference::new(MediaKind::Image, MediaConversation::Group { uin: 42 });
    Message {
        conversation: Conversation::Group { uin: 42 },
        sender_uin: 7,
        sender_uid: "u_peer".into(),
        recipient_uin: 42,
        recipient_uid: String::new(),
        sender_name: "默认".into(),
        sequence: 15,
        client_sequence: 7,
        message_id: 9,
        random: 4,
        time: 1_700_000_000,
        is_self: false,
        is_bot: None,
        elements: vec![
            MessageElement::Text("文本".into()),
            MessageElement::Mention(Mention {
                uin: 7,
                uid: Some("u_peer".into()),
                display: "名字".into(),
                is_everyone: false,
            }),
            MessageElement::Reply(Reply {
                sequences: vec![14],
                message_id: 8,
                sender_uin: 7,
                sender_uid: "u_peer".into(),
                time: 123,
                elements: vec![vec![1, 2, 255]],
            }),
            MessageElement::Image(reference.clone()),
            MessageElement::Voice(reference.clone()),
            MessageElement::Video(reference.clone()),
            MessageElement::File(reference),
            MessageElement::LightApp("{\"prompt\":\"卡片\"}".into()),
            MessageElement::Forward(ForwardReference {
                resource_id: "resource".into(),
                light_app_json: None,
                count: 2,
                raw_element: Some(vec![1, 2]),
            }),
            MessageElement::GroupFile(GroupFileMessage {
                business_id: 1,
                file_id: "file".into(),
                name: "测试.txt".into(),
                size: 99,
                md5: "abcd".into(),
                raw_element: vec![3, 4],
            }),
            MessageElement::Opaque(OpaqueElement {
                scope: OpaqueScope::Element,
                field: 999,
                payload: vec![0, 255, 17],
            }),
        ],
        metadata: vec![OpaqueElement {
            scope: OpaqueScope::MessageBody,
            field: 888,
            payload: vec![5, 6],
        }],
        raw: vec![0, 1, 2, 255],
    }
}
#[tokio::test]
async fn all_content_and_wire_bytes_survive_database_reopen() {
    let root = tempfile::tempdir().unwrap();
    let expected = message();
    let key = conversation_key(&expected.conversation);
    let db = Database::open(root.path()).unwrap();
    db.save_message(1, &expected).await.unwrap();
    db.save_message(1, &expected).await.unwrap();
    db.save_message(2, &expected).await.unwrap();
    db.close().await.unwrap();
    drop(db);
    let db = Database::open(root.path()).unwrap();
    let rows = db.messages(1, &key, 30).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        serde_json::to_value(&rows[0]).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(db.messages(2, &key, 30).unwrap().len(), 1);
    assert!(db.messages(3, &key, 30).unwrap().is_empty());
    db.close().await.unwrap();
}
#[tokio::test]
async fn database_history_limit_keeps_newest_rows() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path()).unwrap();
    let mut item = message();
    let key = conversation_key(&item.conversation);
    for sequence in 1..=5 {
        item.sequence = sequence;
        item.time = sequence as i64;
        db.save_message(1, &item).await.unwrap();
    }
    let rows = db.messages(1, &key, 2).unwrap();
    assert_eq!(
        rows.iter()
            .map(|message| message.sequence)
            .collect::<Vec<_>>(),
        vec![4, 5]
    );
    db.close().await.unwrap();
}
fn friend() -> Friend {
    Friend {
        uin: 7,
        uid: "u_peer".into(),
        nickname: "好友".into(),
        remark: "备注".into(),
        personal_sign: String::new(),
        qid: String::new(),
        age: 0,
        gender: Gender::Unknown(0),
        category_id: 0,
        category: None,
    }
}
fn member() -> GroupMember {
    GroupMember {
        group_uin: 42,
        uin: 7,
        uid: "u_peer".into(),
        nickname: "群昵称".into(),
        permission: GroupMemberPermission::Member,
        level: 0,
        card: Some("群名片".into()),
        special_title: None,
        join_time: 0,
        last_message_time: 0,
        mute_until: 0,
    }
}
#[test]
fn names_follow_contact_then_group_then_default_priority() {
    let mut friend = friend();
    let mut member = member();
    assert_eq!(
        display_name(Some(&friend), Some(&member), "默认", 7),
        "好友"
    );
    friend.nickname.clear();
    assert_eq!(
        display_name(Some(&friend), Some(&member), "默认", 7),
        "群名片"
    );
    member.card = None;
    assert_eq!(display_name(None, Some(&member), "默认", 7), "群昵称");
    member.nickname.clear();
    assert_eq!(display_name(None, Some(&member), "默认", 7), "默认");
    assert_eq!(display_name(None, None, "", 7), "7");
}
#[test]
fn cached_and_live_messages_deduplicate_without_unread() {
    let mut model = Model::default();
    let item = message();
    let key = conversation_key(&item.conversation);
    model.receive(item.clone(), false);
    model.receive(item, false);
    assert_eq!(model.messages[&key].len(), 1);
    assert_eq!(model.chats[0].unread, 0);
}

#[test]
fn acknowledgement_merges_with_server_message() {
    let mut model = Model::default();
    let full = message();
    let key = conversation_key(&full.conversation);
    let mut acknowledgement = full.clone();
    acknowledgement.message_id = 0;
    acknowledgement.raw.clear();
    acknowledgement.metadata.clear();
    model.receive(acknowledgement.clone(), false);
    model.receive(full.clone(), true);
    model.receive(acknowledgement, false);
    assert_eq!(model.messages[&key].len(), 1);
    assert_eq!(model.messages[&key][0].raw, full.raw);
    let mut another = full;
    another.message_id += 1;
    model.receive(another, true);
    assert_eq!(model.messages[&key].len(), 2);
}
#[test]
fn own_group_message_uses_group_name() {
    let mut model = Model::default();
    model.members.insert(42, vec![member()]);
    let mut item = message();
    item.is_self = true;
    assert_eq!(model.sender_name(&item), "群名片");
    model.friends.push(friend());
    assert_eq!(model.sender_name(&item), "好友");
}

#[test]
fn self_echo_with_different_server_id_merges_in_both_orders() {
    for echo_first in [false, true] {
        let mut model = Model::default();
        let mut echo = message();
        echo.is_self = true;
        echo.message_id = 0x0100000000000004;
        let mut ack = echo.clone();
        ack.message_id = 0x1000000000000004;
        ack.sequence = 12345;
        ack.raw.clear();
        ack.metadata.clear();
        ack.time = 0;
        ack.sender_uid.clear();
        let key = conversation_key(&echo.conversation);
        let (first, second) = if echo_first {
            (echo.clone(), ack)
        } else {
            (ack, echo.clone())
        };
        model.receive(first, true);
        model.receive(second, false);
        assert_eq!(model.messages[&key].len(), 1);
        assert_eq!(model.messages[&key][0].message_id, echo.message_id);
        assert_eq!(model.messages[&key][0].raw, echo.raw);
        let mut another = echo;
        another.message_id += 1;
        another.sequence += 1;
        another.random += 1;
        another.client_sequence += 1;
        model.receive(another, true);
        assert_eq!(model.messages[&key].len(), 2);
    }
}
#[tokio::test]
async fn login_restores_recent_messages_and_discovers_orphaned_conversations() {
    use ferrsphen::model::Chat;
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path()).unwrap();
    let mut group = message();
    db.put(
        "chats/1",
        &vec![Chat {
            conversation: group.conversation.clone(),
            title: "群".into(),
            draft: "草稿".into(),
            unread: 2,
        }],
    )
    .await
    .unwrap();
    for sequence in 1..=305 {
        group.sequence = sequence;
        group.message_id = sequence;
        group.time = sequence as i64;
        db.save_message(1, &group).await.unwrap();
    }
    let mut private = message();
    private.conversation = Conversation::Friend {
        uin: 7,
        uid: "u_peer".into(),
    };
    private.time = 400;
    db.save_message(1, &private).await.unwrap();
    let mut legacy = private.clone();
    legacy.conversation = Conversation::Group { uin: 99 };
    legacy.time = 500;
    db.put("message/1/group:99/00000000000000000500/1/1", &legacy)
        .await
        .unwrap();
    db.save_message(2, &message()).await.unwrap();
    db.close().await.unwrap();
    drop(db);
    let db = Database::open(root.path()).unwrap();
    let restored = db.restore(1).unwrap();
    assert_eq!(restored.chats.len(), 3);
    assert_eq!(restored.selected.as_deref(), Some("group:99"));
    assert_eq!(restored.messages["group:42"].len(), 300);
    assert_eq!(restored.messages["group:42"][0].sequence, 6);
    assert_eq!(restored.messages["dm:uid:u_peer"][0].time, 400);
    let chat = restored
        .chats
        .iter()
        .find(|chat| conversation_key(&chat.conversation) == "group:42")
        .unwrap();
    assert_eq!(chat.draft, "草稿");
    assert_eq!(chat.title, "群");
    assert_eq!(chat.unread, 2);
    assert_eq!(db.restore(2).unwrap().chats.len(), 1);
    assert!(db.restore(3).unwrap().chats.is_empty());
    db.close().await.unwrap();
}
#[tokio::test]
async fn persisted_ack_and_echo_restore_as_one_message() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path()).unwrap();
    let mut echo = message();
    echo.is_self = true;
    let mut ack = echo.clone();
    ack.raw.clear();
    ack.message_id += 100;
    ack.sequence += 100;
    db.save_message(1, &ack).await.unwrap();
    db.save_message(1, &echo).await.unwrap();
    let model = db.restore(1).unwrap();
    assert_eq!(model.messages["group:42"].len(), 1);
    assert_eq!(model.messages["group:42"][0].raw, echo.raw);
    db.close().await.unwrap();
}

#[test]
fn different_nonces_preserve_repeated_messages() {
    let mut model = Model::default();
    let mut first = message();
    first.is_self = true;
    first.raw.clear();
    let mut second = first.clone();
    second.message_id += 1;
    second.random += 1;
    let key = conversation_key(&first.conversation);
    model.receive(first, false);
    model.receive(second, false);
    assert_eq!(model.messages[&key].len(), 2);
}

#[test]
fn group_echo_uid_recovers_nonce_when_random_header_is_absent() {
    let mut ack = message();
    ack.is_self = true;
    ack.message_id = (0x10000000u64 << 32) | ack.random as u64;
    ack.raw.clear();
    let mut echo = ack.clone();
    echo.message_id = (0x01000000u64 << 32) | echo.random as u64;
    echo.random = 0;
    echo.client_sequence = 0;
    echo.sequence += 42;
    echo.raw = vec![1, 2, 3];
    let mut model = Model::default();
    model.receive(ack, false);
    model.receive(echo, true);
    assert_eq!(model.messages["group:42"].len(), 1);
    assert_eq!(model.messages["group:42"][0].raw, vec![1, 2, 3]);
}

#[test]
fn optimistic_message_is_visible_before_receipt_and_reorders_on_confirmation() {
    for echo_first in [false, true] {
        let mut model = Model::default();
        let mut echo = message();
        echo.is_self = true;
        echo.random = 1234;
        echo.message_id = (0x01000000u64 << 32) | 1234;
        model.stage(
            1,
            echo.conversation.clone(),
            echo.elements.clone(),
            1234,
            echo.time + 10,
        );
        assert_eq!(model.messages["group:42"].len(), 1);
        assert!(model.optimistic.contains_key(&1));
        let mut neighbour = echo.clone();
        neighbour.is_self = false;
        neighbour.message_id += 10;
        neighbour.sequence += 10;
        neighbour.random += 10;
        neighbour.time += 5;
        model.receive(neighbour, true);
        if echo_first {
            model.receive(echo.clone(), true);
        }
        let mut ack = echo.clone();
        ack.raw.clear();
        ack.message_id = (0x10000000u64 << 32) | 1234;
        model.confirm(1, ack);
        model.receive(echo.clone(), true);
        assert_eq!(model.messages["group:42"].len(), 2);
        assert_eq!(model.messages["group:42"][0].message_id, echo.message_id);
        assert!(!model.optimistic.contains_key(&1));
    }
}
#[test]
fn recall_eligibility_expires_at_two_minutes() {
    use ferrsphen::model::can_recall;
    let mut item = message();
    item.is_self = true;
    assert!(can_recall(&item, item.time + 119));
    assert!(!can_recall(&item, item.time + 120));
    assert!(!can_recall(&item, item.time - 1));
    item.is_self = false;
    assert!(!can_recall(&item, item.time));
    item.is_self = true;
    item.sequence = 0;
    assert!(!can_recall(&item, item.time));
}
#[tokio::test]
async fn recall_marker_survives_restart_without_erasing_message() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path()).unwrap();
    let item = message();
    db.save_message(1, &item).await.unwrap();
    db.save_recall(1, &item.conversation, item.sequence)
        .await
        .unwrap();
    db.close().await.unwrap();
    drop(db);
    let db = Database::open(root.path()).unwrap();
    let model = db.restore(1).unwrap();
    assert!(model.recalled.contains("group:42:15"));
    assert_eq!(model.messages["group:42"].len(), 1);
    assert_eq!(model.messages["group:42"][0].raw, item.raw);
    assert!(db.restore(2).unwrap().recalled.is_empty());
    db.close().await.unwrap();
}
#[test]
fn forward_card_preview_does_not_depend_on_count() {
    let reference = ForwardReference {
        resource_id: "record".into(),
        count: 0,
        raw_element: None,
        light_app_json: Some(
            r#"{"meta":{"detail":{"news":[{"text":"A: hello"},{"text":"B: hi"}]}}}"#.into(),
        ),
    };
    assert_eq!(
        ferrsphen::viewer::forward_preview(&reference),
        vec!["A: hello", "B: hi"]
    );
    assert_eq!(
        ferrsphen::model::summary(&[MessageElement::Forward(reference)]),
        "[聊天记录]"
    );
}

#[test]
fn conversation_order_tracks_activity_including_optimistic_sends() {
    let mut model = Model::default();
    let mut first = message();
    first.elements = vec![MessageElement::Text("first".into())];
    let mut second = first.clone();
    second.conversation = Conversation::Group { uin: 43 };
    second.time += 10;
    model.receive(first.clone(), false);
    model.receive(second, false);
    let keys = |model: &Model| {
        model
            .sorted_chats()
            .iter()
            .map(|chat| conversation_key(&chat.conversation))
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&model), ["group:43", "group:42"]);
    model.stage(
        1,
        first.conversation.clone(),
        vec![MessageElement::Text("new".into())],
        88,
        first.time + 20,
    );
    assert_eq!(keys(&model), ["group:42", "group:43"]);
    let mut older = first;
    older.sequence = 1;
    older.message_id = 1;
    older.time -= 1000;
    model.receive(older, false);
    assert_eq!(keys(&model), ["group:42", "group:43"]);
}

fn picture(sequence: u64, sender: u64) -> Message {
    let mut item = message();
    item.sequence = sequence;
    item.message_id = sequence;
    item.sender_uin = sender;
    item.sender_uid = format!("peer-{sender}");
    item.elements = vec![
        item.elements
            .iter()
            .find(|element| matches!(element, MessageElement::Image(_)))
            .unwrap()
            .clone(),
    ];
    item
}

#[test]
fn image_groups_respect_sender_text_and_recall_boundaries() {
    use ferrsphen::model::message_groups;
    let mut rich = picture(3, 7);
    rich.elements.push(MessageElement::Text("caption".into()));
    rich.elements.push(rich.elements[0].clone());
    let mut multi = picture(7, 7);
    multi.elements.push(multi.elements[0].clone());
    let rows = vec![
        picture(1, 7),
        picture(2, 7),
        rich,
        picture(4, 7),
        picture(5, 8),
        picture(6, 7),
        multi,
    ];
    let mut recalled = std::collections::HashSet::new();
    let sizes = |recalled: &std::collections::HashSet<String>| {
        message_groups(&rows, recalled)
            .iter()
            .map(|group| group.len())
            .collect::<Vec<_>>()
    };
    assert_eq!(sizes(&recalled), [2, 1, 1, 1, 1, 1]);
    recalled.insert("group:42:2".into());
    assert_eq!(sizes(&recalled), [1, 1, 1, 1, 1, 1, 1]);
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[6].elements.len(), 2);
    assert!(message_groups(&[], &recalled).is_empty());
}

#[test]
fn quoted_content_roundtrips_without_loaded_history() {
    let mut item = picture(15, 7);
    if let MessageElement::Image(image) = &mut item.elements[0] {
        image.compatibility = Some(vec![0x12, 5, b'p', b'h', b'o', b't', b'o']);
    }
    item.elements.push(MessageElement::Text("caption".into()));
    let reply = Reply::with_preview(&item).unwrap();
    assert!(!reply.elements.is_empty());
    let encoded = atri_qq::messages::encode_elements(
        &[
            MessageElement::Reply(reply.clone()),
            MessageElement::Text("reply".into()),
        ],
        &[],
    )
    .unwrap();
    let decoded = atri_qq::messages::decode_elements(&encoded, &item.conversation, 1).unwrap();
    let MessageElement::Reply(reply) = &decoded.elements[0] else {
        panic!("missing quoted content")
    };
    let mut model = Model::default();
    model.members.insert(
        42,
        vec![GroupMember {
            uin: 7,
            uid: "peer-7".into(),
            nickname: "昵称".into(),
            card: Some("群昵称".into()),
            ..member()
        }],
    );
    let preview = model.reply_preview(&item.conversation, reply);
    assert_eq!(model.sender_name(&preview), "群昵称");
    assert!(
        preview
            .elements
            .iter()
            .any(|element| matches!(element, MessageElement::Image(_)))
    );
    assert!(
        preview
            .elements
            .iter()
            .any(|element| matches!(element, MessageElement::Text(text) if text == "caption"))
    );
    model.friends.push(Friend {
        uid: "peer-7".into(),
        ..friend()
    });
    assert_eq!(model.sender_name(&preview), "好友");
    model.receive(item.clone(), false);
    let mut impostor = item.clone();
    impostor.sender_uid = "another".into();
    impostor.sender_uin = 99;
    assert!(ferrsphen::model::reply_target(&[impostor], reply).is_none());
    assert_eq!(model.reply_preview(&item.conversation, reply).raw, item.raw);
}

fn historical_tombstone() -> Message {
    let mut item = message();
    item.elements.clear();
    item.metadata.clear();
    item.raw = vec![0x0a, 6, 8, 7, 0x42, 2, 8, 42, 0x12, 4, 8, 82, 0x28, 15];
    item
}
#[test]
fn history_tombstone_marks_recall_even_when_richer_cached_content_is_kept() {
    let mut model = Model::default();
    let mut original = message();
    original.raw = vec![1; 100];
    model.receive(original.clone(), false);
    model.receive(historical_tombstone(), false);
    assert_eq!(model.messages["group:42"].len(), 1);
    assert!(model.is_recalled(&model.messages["group:42"][0]));
    assert_eq!(model.messages["group:42"][0].raw, original.raw);
    model.receive(original, false);
    assert!(model.is_recalled(&model.messages["group:42"][0]));
}
#[tokio::test]
async fn history_recall_survives_restart_and_an_older_cache_write() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path()).unwrap();
    db.save_message(1, &historical_tombstone()).await.unwrap();
    assert_eq!(
        db.get::<String>(&format!("recall/1/{}", hex::encode("group:42:15")))
            .unwrap()
            .as_deref(),
        Some("group:42:15")
    );
    db.save_message(1, &message()).await.unwrap();
    db.close().await.unwrap();
    drop(db);
    let db = Database::open(root.path()).unwrap();
    let model = db.restore(1).unwrap();
    assert_eq!(model.messages["group:42"].len(), 1);
    assert!(model.is_recalled(&model.messages["group:42"][0]));
    assert!(db.restore(2).unwrap().recalled.is_empty());
    db.close().await.unwrap();
}
#[tokio::test]
async fn legacy_cached_tombstone_is_recognized_without_a_saved_recall_event() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path()).unwrap();
    let item = historical_tombstone();
    db.put(
        &format!(
            "message/1/{}/{:020}/{:020}/{:020}",
            hex::encode("group:42"),
            item.time,
            item.sequence,
            item.message_id
        ),
        &item,
    )
    .await
    .unwrap();
    let model = db.restore(1).unwrap();
    assert!(model.is_recalled(&model.messages["group:42"][0]));
    db.close().await.unwrap();
}

#[tokio::test]
async fn private_recall_matches_server_sequence_after_restoring_legacy_wire_identity() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path()).unwrap();
    let mut item = message();
    item.conversation = Conversation::Friend {
        uin: 7,
        uid: "peer".into(),
    };
    item.sequence = 444;
    item.client_sequence = 333;
    item.raw = vec![
        0x0a, 2, 8, 7, 0x12, 9, 8, 0xa6, 1, 0x28, 0xbc, 3, 0x58, 0xcd, 2,
    ];
    db.save_message(1, &item).await.unwrap();
    db.save_recall(
        1,
        &Conversation::Friend {
            uin: 0,
            uid: "peer".into(),
        },
        333,
    )
    .await
    .unwrap();
    let model = db.restore(1).unwrap();
    let restored = &model.messages["dm:uid:peer"][0];
    assert_eq!((restored.sequence, restored.client_sequence), (333, 444));
    assert!(model.is_recalled(restored));
    assert_eq!(Reply::from(restored).sequences, [444]);
    assert!(
        ferrsphen::model::reply_target(&model.messages["dm:uid:peer"], &Reply::from(restored))
            .is_some()
    );
    db.close().await.unwrap();
}

#[tokio::test]
async fn avatar_identity_cache_survives_restart_and_is_account_scoped() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path()).unwrap();
    assert!(db.user_uin(1, "peer").unwrap().is_none());
    db.save_user_uin(1, "peer", 77).await.unwrap();
    assert!(db.save_user_uin(1, "bad", 0).await.is_err());
    db.close().await.unwrap();
    drop(db);
    let db = Database::open(root.path()).unwrap();
    assert_eq!(db.user_uin(1, "peer").unwrap(), Some(77));
    assert!(db.user_uin(2, "peer").unwrap().is_none());
    db.close().await.unwrap();
}

#[test]
fn conversation_preview_only_prefixes_other_group_members() {
    let mut model = Model::default();
    let mut row = message();
    row.elements = vec![MessageElement::Text("消息".into())];
    let group = row.conversation.clone();
    model.receive(row.clone(), false);
    assert_eq!(model.chat_preview(&group), "默认: 消息");
    model.members.insert(42, vec![member()]);
    assert_eq!(model.chat_preview(&group), "群名片: 消息");
    model.friends.push(friend());
    assert_eq!(model.chat_preview(&group), "好友: 消息");
    model.messages.get_mut("group:42").unwrap()[0].is_self = true;
    assert_eq!(model.chat_preview(&group), "消息");
    row.conversation = Conversation::Friend {
        uin: 7,
        uid: "u_peer".into(),
    };
    let direct = row.conversation.clone();
    model.receive(row, false);
    assert_eq!(model.chat_preview(&direct), "消息");
}
