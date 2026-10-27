//! Example to send medias already on Telegram server as a media group, and add spoiler to them.
//!
//! The `TG_ID` and `TG_HASH` environment variables must be set (learn how to do it for
//! [Windows](https://ss64.com/nt/set.html) or [Linux](https://ss64.com/bash/export.html))
//! to Telegram's API ID and API hash respectively.
//!
//! Then, run it as:
//!
//! ```sh
//! cargo run --example send_album
//! ```

use std::env;
use std::io;
use std::io::{BufRead, Write};
use std::sync::Arc;

use grammers_client::Client;
use grammers_client::SignInError;
use grammers_client::media::InputMedia;
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use grammers_session::types::PeerId;
use grammers_tl_types as tl;
use tokio::runtime;
use tracing::info;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const SESSION_FILE: &str = "send_album.session";

async fn async_main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .without_time()
        .init();

    let api_id: i32 = env!("TG_ID").parse().expect("TG_ID invalid");
    let api_hash: String = env!("TG_HASH").to_string();

    let session = Arc::new(SqliteSession::open(SESSION_FILE).await?);

    let SenderPool { runner, handle, .. } = SenderPool::new(Arc::clone(&session), api_id);
    let client = Client::new(handle);
    let _ = tokio::spawn(runner.run());

    if !client.is_authorized().await? {
        info!("Signing in...");
        let phone = prompt("Enter your phone number (international format): ")?;
        let token = client.request_login_code(&phone, &api_hash).await?;
        let code = prompt("Enter the code you received: ")?;
        let signed_in = client.sign_in(&token, &code).await;
        match signed_in {
            Err(SignInError::PasswordRequired(password_token)) => {
                // Note: this `prompt` method will echo the password in the console.
                //       Real code might want to use a better way to handle this.
                let hint = password_token.hint().unwrap_or("");
                let prompt_message = format!("Enter the password (hint {}): ", &hint);
                let password = prompt(prompt_message.as_str())?;

                client
                    .check_password(password_token, password.trim())
                    .await?;
            }
            Ok(_) => (),
            Err(e) => panic!("{}", e),
        };
        info!("Signed in!");
    }

    // In a production environment, it may be necessary to cache the peer ID corresponding to the username,
    // to avoid flood waits.
    let peer = client
        .resolve_username("telegram")
        .await?
        .ok_or("no peer with username")?
        .to_ref()
        .await?
        .unwrap();

    // Both users and bots can use get_messages_by_id.
    let messages = client.get_messages_by_id(peer, &[437, 438]).await?;
    let medias = messages
        .into_iter()
        .map(|message| {
            let media = message
                .unwrap()
                .media()
                .unwrap()
                .to_raw_input_media()
                .unwrap();
            let (id, video_cover, video_timestamp) = match media {
                tl::enums::InputMedia::Document(x) => (x.id, x.video_cover, x.video_timestamp),
                _ => panic!("must be document"),
            };
            let media = tl::types::InputMediaDocument {
                spoiler: true,
                id,
                video_cover,
                video_timestamp,
                ttl_seconds: None,
                query: None,
            };
            InputMedia::new().media(media)
        })
        .collect();

    let me = PeerId::self_user().to_ambient_ref();
    let messages = client.send_album(me, medias).await?;
    info!(?messages);

    // `runner.run()`'s task will be dropped (and disconnect occur) once the runtime exits.
    Ok(())
}

fn main() -> Result<()> {
    runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main())
}

fn prompt(message: &str) -> Result<String> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    stdout.write_all(message.as_bytes())?;
    stdout.flush()?;

    let stdin = io::stdin();
    let mut stdin = stdin.lock();

    let mut line = String::new();
    stdin.read_line(&mut line)?;
    Ok(line)
}
