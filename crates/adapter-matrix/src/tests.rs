use super::*;
use chat_adapter::mock::MemoryStore;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path_regex},
};

fn input(step: &str, fields: &[(&str, &str)]) -> AuthInput {
    AuthInput {
        step: step.into(),
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}
async fn server(flows: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"/_matrix/client/versions$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"versions":["v1.11"]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/_matrix/client/(r0|v3)/login$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"flows":flows})))
        .mount(&server)
        .await;
    server
}
#[tokio::test]
async fn sdk_discovery_drives_common_login_forms_and_preserves_password_login_errors() {
    let server = server(
        json!([{"type":"m.login.password"},{"type":"m.login.sso"},{"type":"m.login.token"}]),
    )
    .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/_matrix/client/(r0|v3)/login$"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"errcode":"M_FORBIDDEN","error":"Invalid credentials"})),
        )
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let adapter = MatrixAdapter::new(
        "one",
        Arc::new(MemoryStore::default()),
        directory.path().into(),
    );
    assert!(
        matches!(adapter.authenticate(input("resume", &[])).await.unwrap(),AuthState::Form{step,..} if step=="homeserver")
    );
    let state = adapter
        .authenticate(input("homeserver", &[("homeserver", &server.uri())]))
        .await
        .unwrap();
    let AuthState::Choice { choices, .. } = state else {
        panic!("expected server-advertised choices")
    };
    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.id.as_str())
            .collect::<Vec<_>>(),
        ["password", "sso", "token"]
    );
    let AuthState::Form { step, fields } = adapter
        .authenticate(input("method", &[("choice", "password")]))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(step, "password");
    assert!(
        fields
            .iter()
            .any(|field| field.id == "password" && field.kind == FieldKind::Secret)
    );
    assert!(
        adapter
            .authenticate(input("password", &[("username", "alice")]))
            .await
            .is_err()
    );
    let error = adapter
        .authenticate(input(
            "password",
            &[("username", "alice"), ("password", "invalid-test-password")],
        ))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Invalid credentials"), "{error}");
    let requests = server.received_requests().await.unwrap();
    let request = requests
        .iter()
        .find(|r| r.method == "POST" && r.url.path().ends_with("/login"))
        .unwrap();
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["type"], "m.login.password");
    assert_eq!(body["identifier"]["user"], "alice");
    adapter.close().await.unwrap();
}
#[tokio::test]
async fn restored_sessions_are_bound_to_the_local_account_and_shutdown_without_logout() {
    let server = server(json!([{"type":"m.login.password"}])).await;
    Mock::given(method("GET"))
        .and(path_regex(r"/_matrix/client/(r0|v3)/account/whoami$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"user_id":"@alice:example.org","device_id":"TEST"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(
            r"/_matrix/client/(r0|v3)/profile/.*/displayname$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"displayname":"Alice"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(
            r"/_matrix/client/(r0|v3)/profile/.*/avatar_url$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/_matrix/client/(r0|v3)/sync$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(
                    json!({"next_batch":"test-sync","rooms":{},"to_device":{"events":[]}}),
                )
                .set_delay(Duration::from_secs(2)),
        )
        .mount(&server)
        .await;
    let store = Arc::new(MemoryStore::default());
    let directory = tempfile::tempdir().unwrap();
    let adapter = MatrixAdapter::new("one", store.clone(), directory.path().join("one"));
    store.put(adapter.key("session"),json!({"homeserver":server.uri(),"session":{"access_token":"test-only-token","user_id":"@alice:example.org","device_id":"TEST"}})).await.unwrap();
    let state = adapter.authenticate(input("resume", &[])).await.unwrap();
    assert!(
        matches!(state,AuthState::Ready{user,..} if user.id=="@alice:example.org" && user.name=="Alice")
    );
    assert_eq!(
        store.get(adapter.key("user")).await.unwrap(),
        Some(json!("@alice:example.org"))
    );
    // A second local slot cannot resume or reuse the first slot's session.
    let second = MatrixAdapter::new("two", store.clone(), directory.path().join("two"));
    assert!(matches!(
        second.authenticate(input("resume", &[])).await.unwrap(),
        AuthState::Form { .. }
    ));
    adapter.close().await.unwrap();
    second.close().await.unwrap();
    assert!(store.get(adapter.key("session")).await.unwrap().is_some());
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path().ends_with("/logout"))
    );
}
#[tokio::test]
async fn unknown_homeserver_methods_are_explicit_and_never_silently_password_login() {
    let server = server(json!([{"type":"org.example.unsupported"}])).await;
    let directory = tempfile::tempdir().unwrap();
    let adapter = MatrixAdapter::new(
        "one",
        Arc::new(MemoryStore::default()),
        directory.path().into(),
    );
    assert!(matches!(
        adapter
            .authenticate(input("homeserver", &[("homeserver", &server.uri())]))
            .await,
        Err(Error::Unsupported(_))
    ));
    adapter.close().await.unwrap();
}

#[tokio::test]
async fn official_sdk_sync_history_and_text_send_use_adapter_ids_and_transaction_ids() {
    let server = server(json!([{"type":"m.login.password"}])).await;
    let own = json!({"type":"m.room.member","state_key":"@alice:example.org","sender":"@alice:example.org","event_id":"$join","origin_server_ts":1,"content":{"membership":"join","displayname":"Alice"}});
    let message = json!({"type":"m.room.message","sender":"@bob:example.org","event_id":"$hello","origin_server_ts":2,"content":{"msgtype":"m.text","body":"Hello"}});
    Mock::given(method("GET")).and(path_regex(r"/_matrix/client/(r0|v3)/sync$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"next_batch":"batch","rooms":{"join":{"!room:example.org":{"state":{"events":[own]},"timeline":{"events":[message.clone()],"limited":false,"prev_batch":"before"}}}},"to_device":{"events":[]}}))).mount(&server).await;
    Mock::given(method("PUT"))
        .and(path_regex(
            r"/_matrix/client/(r0|v3)/rooms/.*/send/m.room.message/local-nonce$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"event_id":"$sent"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/_matrix/client/(r0|v3)/rooms/.*/messages$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"chunk":[message],"start":"batch","end":"older"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(
            r"/_matrix/client/(r0|v3)/rooms/.*/state/m.room.encryption/?$",
        ))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"errcode":"M_NOT_FOUND","error":"Room is not encrypted"})),
        )
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let adapter = MatrixAdapter::new(
        "slot",
        Arc::new(MemoryStore::default()),
        directory.path().into(),
    );
    let client = adapter.build(&server.uri()).await.unwrap();
    client.restore_session(serde_json::from_value::<MatrixSession>(json!({"access_token":"test-only-token","user_id":"@alice:example.org","device_id":"TEST"})).unwrap()).await.unwrap();
    client.sync_once(SyncSettings::default()).await.unwrap();
    {
        let mut state = adapter.state.lock().await;
        state.client = Some(client);
        state.ready = true;
    }
    let rooms = adapter.conversations(PageRequest::default()).await.unwrap();
    assert_eq!(rooms.items.len(), 1);
    let chat = rooms.items[0].id.clone();
    assert_eq!(chat.account.id, "slot");
    let history = adapter
        .history(chat.clone(), PageRequest::default())
        .await
        .unwrap();
    assert_eq!(
        history.items[0].content,
        vec![Content::Text("Hello".into())]
    );
    assert_eq!(history.next.as_deref(), Some("older"));
    let sent = adapter
        .send(SendRequest {
            chat: chat.clone(),
            client_id: "local-nonce".into(),
            content: vec![Content::Text("Reply".into())],
            reply_to: Some(history.items[0].id.clone()),
        })
        .await;
    if let Err(error) = &sent {
        panic!(
            "{error}: {:?}",
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .map(|r| r.url.path())
                .collect::<Vec<_>>()
        );
    }
    let sent = sent.unwrap();
    let SendOutcome::Confirmed(sent) = sent else {
        panic!()
    };
    assert_eq!(sent.id.id, "$sent");
    assert_eq!(sent.client_id.as_deref(), Some("local-nonce"));
    let requests = server.received_requests().await.unwrap();
    let send = requests.iter().find(|r| r.method == "PUT").unwrap();
    let body: Value = serde_json::from_slice(&send.body).unwrap();
    assert_eq!(body["body"], "Reply");
    assert_eq!(body["m.relates_to"]["m.in_reply_to"]["event_id"], "$hello");
    adapter.close().await.unwrap();
}
