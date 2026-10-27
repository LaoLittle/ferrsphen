//! Verify an encrypted MTProto connection without signing into a Telegram account.
use adapter_telegram::TdTransport;
use grammers_mtsender::{ServerAddr, connect};
use grammers_tl_types as tl;
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "149.154.167.50:443".into())
        .parse()?;
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let mut sender = connect(TdTransport::default(), ServerAddr::Tcp { address }).await?;
        let ping_id = 0x465252535048454e;
        let tl::enums::Pong::Pong(pong) = sender.invoke(&tl::functions::Ping { ping_id }).await?;
        if pong.ping_id != ping_id {
            return Err("Pong ID mismatch".into());
        }
        println!("Encrypted MTProto Pong verified: peer={address}");
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}
