use anyhow::Result;
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init(root: &Path) -> Result<PathBuf> {
    let directory = root.join("logs");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!(
        "packets-{}-{}.jsonl",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ"),
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    let console_filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(console_filter))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                    metadata.target() == "atri_qq_protocol::packet_trace"
                })),
        )
        .try_init()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn packet_file_retains_full_payload_at_debug_level() {
        let root = tempfile::tempdir().unwrap();
        let path = super::init(root.path()).unwrap();
        let body = "abcd".repeat(32 * 1024);
        tracing::debug!(target: "atri_qq_protocol::packet_trace", direction = "send", command = "test", body_hex = %body, "QQ packet");
        tracing::debug!(target: "atri_qq_protocol::packet_trace", direction = "receive", command = "test", body_hex = %body, "QQ packet");
        let logs = std::fs::read_to_string(path).unwrap();
        let packets = logs
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|record| record["fields"]["command"] == "test")
            .collect::<Vec<_>>();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0]["fields"]["direction"], "send");
        assert_eq!(packets[1]["fields"]["direction"], "receive");
        for packet in packets {
            assert_eq!(packet["fields"]["body_hex"], body);
        }
    }
}
