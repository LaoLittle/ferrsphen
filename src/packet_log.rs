use anyhow::Result;
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init(root: &Path) -> Result<PathBuf> {
    init_with_logger(root, cfg!(target_os = "android"))
}

fn init_with_logger(root: &Path, preserve_native_logger: bool) -> Result<PathBuf> {
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
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(console_filter))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                    metadata.target() == "atri_qq_protocol::packet_trace"
                })),
        );
    if preserve_native_logger {
        tracing::subscriber::set_global_default(subscriber)?;
    } else {
        subscriber.try_init()?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn packet_initialization_preserves_existing_native_logger() {
        const CHILD: &str = "FERRSPHEN_NATIVE_LOGGER_TEST";
        if std::env::var_os(CHILD).is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "packet_log::tests::packet_initialization_preserves_existing_native_logger",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct NativeLogger(AtomicUsize);
        impl log::Log for NativeLogger {
            fn enabled(&self, _: &log::Metadata<'_>) -> bool {
                true
            }
            fn log(&self, _: &log::Record<'_>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
            fn flush(&self) {}
        }
        static LOGGER: NativeLogger = NativeLogger(AtomicUsize::new(0));
        log::set_logger(&LOGGER).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
        log::info!("native logger before startup");
        let root = tempfile::tempdir().unwrap();
        let path = super::init_with_logger(root.path(), true).unwrap();
        let before = LOGGER.0.load(Ordering::Relaxed);
        log::info!("native logger after startup");
        assert!(LOGGER.0.load(Ordering::Relaxed) > before);
        let body = "0123456789abcdef".repeat(4096);
        tracing::debug!(target: "atri_qq_protocol::packet_trace", direction = "receive", command = "native-startup-test", body_hex = %body, "QQ packet");
        let logs = std::fs::read_to_string(path).unwrap();
        let packet = logs
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|record| record["fields"]["command"] == "native-startup-test")
            .unwrap();
        assert_eq!(packet["fields"]["body_hex"], body);
        assert!(super::init_with_logger(root.path(), true).is_err());
    }

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
