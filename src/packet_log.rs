use anyhow::Result;
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tracing_subscriber::{
    Layer,
    fmt::{format::Writer, time::FormatTime},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

struct LocalTimestamp;
impl FormatTime for LocalTimestamp {
    fn format_time(&self, writer: &mut Writer<'_>) -> std::fmt::Result {
        write!(
            writer,
            "{}",
            chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
        )
    }
}

// The protocol currently logs this normal pagination status at ERROR. Only
// filter this exact diagnostic on the console; packet_trace remains complete.
struct HistoryStatusFilter;
impl<S: tracing::Subscriber> tracing_subscriber::layer::Filter<S> for HistoryStatusFilter {
    fn enabled(
        &self,
        _: &tracing::Metadata<'_>,
        _: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        true
    }
    fn event_enabled(
        &self,
        event: &tracing::Event<'_>,
        _: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        if event.metadata().target() != "atri_qq_protocol::packet" {
            return true;
        }
        #[derive(Default)]
        struct Status {
            command: bool,
            response: bool,
            body_code: bool,
            exhausted: bool,
        }
        impl tracing::field::Visit for Status {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if matches!(field.name(), "command" | "response_type" | "status_field") {
                    self.record_str(field, &format!("{value:?}"));
                }
            }
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                match field.name() {
                    "command" => {
                        self.command =
                            value == "trpc.msg.register_proxy.RegisterProxy.SsoGetGroupMsg"
                    }
                    "response_type" => {
                        self.response = value == "atri.qq.messages.GroupHistoryResponse"
                    }
                    "status_field" => self.body_code = value == "body.code",
                    _ => {}
                }
            }
            fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
                if field.name() == "code" {
                    self.exhausted = value == 4;
                }
            }
            fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
                if field.name() == "code" {
                    self.exhausted = value == 4;
                }
            }
        }
        let mut status = Status::default();
        event.record(&mut status);
        !(status.command && status.response && status.body_code && status.exhausted)
    }
}

pub fn init(root: &Path) -> Result<PathBuf> {
    init_with_logger(root, cfg!(target_os = "android"))
}

fn init_with_logger(root: &Path, preserve_native_logger: bool) -> Result<PathBuf> {
    let directory = root.join("logs");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!(
        "packets-{}-{}.jsonl",
        chrono::Local::now().format("%Y%m%dT%H%M%S%.9f%z"),
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
        .with(
            tracing_subscriber::fmt::layer()
                .with_timer(LocalTimestamp)
                .with_file(true)
                .with_line_number(true)
                .with_filter(console_filter)
                .with_filter(HistoryStatusFilter),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_timer(LocalTimestamp)
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
    fn assert_local_timestamp(timestamp: &str) {
        let parsed = chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
        let now = chrono::Local::now();
        assert_eq!(
            parsed.offset().local_minus_utc(),
            now.offset().local_minus_utc()
        );
        assert!((now.signed_duration_since(parsed).num_seconds()).abs() < 60);
        assert!(!timestamp.ends_with('Z'));
    }
    #[test]
    fn console_timestamp_uses_the_system_local_offset() {
        use tracing_subscriber::fmt::{format::Writer, time::FormatTime};
        let mut output = String::new();
        super::LocalTimestamp
            .format_time(&mut Writer::new(&mut output))
            .unwrap();
        assert_local_timestamp(&output);
    }
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
        let filename = path.file_name().unwrap().to_str().unwrap();
        let local_offset = chrono::Local::now().format("%z").to_string();
        assert!(filename.contains(&local_offset));
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
            assert_local_timestamp(packet["timestamp"].as_str().unwrap());
            assert_eq!(packet["fields"]["body_hex"], body);
        }
    }
}

#[cfg(test)]
mod history_filter_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tracing_subscriber::{Layer, layer::SubscriberExt};
    struct Counter(Arc<AtomicUsize>);
    impl<S: tracing::Subscriber> Layer<S> for Counter {
        fn on_event(&self, _: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    #[test]
    fn filters_only_group_history_body_four_and_retains_packet_trace() {
        let console = Arc::new(AtomicUsize::new(0));
        let packets = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry()
            .with(Counter(console.clone()).with_filter(super::HistoryStatusFilter))
            .with(
                Counter(packets.clone()).with_filter(tracing_subscriber::filter::filter_fn(|m| {
                    m.target() == "atri_qq_protocol::packet_trace"
                })),
            );
        tracing::subscriber::with_default(subscriber, || {
            for (command, field, code) in [
                (
                    "trpc.msg.register_proxy.RegisterProxy.SsoGetGroupMsg",
                    "body.code",
                    4i64,
                ),
                (
                    "trpc.msg.register_proxy.RegisterProxy.SsoGetGroupMsg",
                    "body.code",
                    5,
                ),
                (
                    "trpc.msg.register_proxy.RegisterProxy.SsoGetGroupMsg",
                    "code",
                    4,
                ),
                ("another.command", "body.code", 4),
            ] {
                // Match the protocol's Display-formatted response_type field.
                tracing::error!(target: "atri_qq_protocol::packet", command,
                    response_type = %"atri.qq.messages.GroupHistoryResponse", status_field = field, code,
                    "QQ server error response");
            }
            assert_eq!(console.load(Ordering::Relaxed), 3);
            tracing::debug!(target: "atri_qq_protocol::packet_trace", code = 4, raw_hex = "1a08080418f0a2ac8403");
        });
        assert_eq!(packets.load(Ordering::Relaxed), 1);
    }
}
