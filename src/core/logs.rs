//! One redacted line goes to stdout/journald and the in-process live stream.
use crate::core::config::Config;
use std::{
    fmt,
    io::{self, Write},
    sync::Arc,
};
use tokio::sync::broadcast;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    fmt::{FmtContext, FormatEvent, FormatFields, format::Writer},
    registry::LookupSpan,
};
pub const APP: &str = "cortex";
#[derive(Clone)]
pub struct Redactor {
    values: Arc<Vec<String>>,
}
impl Redactor {
    pub fn new(mut values: Vec<String>) -> Self {
        values.retain(|v| !v.is_empty());
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        Self {
            values: Arc::new(values),
        }
    }
    pub fn from_environment() -> Self {
        Self::new(
            std::env::vars()
                .filter(|(key, _)| {
                    let key = key.to_ascii_uppercase();
                    ["PASSWORD", "SECRET", "TOKEN", "KEY"]
                        .iter()
                        .any(|word| key.contains(word))
                        || key.ends_with("_URL")
                })
                .map(|(_, value)| value)
                .collect(),
        )
    }
    pub fn clean(&self, value: &str) -> String {
        let mut value = value.replace(['\r', '\n'], "\\n");
        for secret in self.values.iter() {
            value = value.replace(secret, "[redacted]");
        }
        static RULES: std::sync::LazyLock<Vec<(regex::Regex, &str)>> = std::sync::LazyLock::new(
            || {
                vec![
            (regex::Regex::new(r"(?i)(bearer\s+)[^\s,;]+").unwrap(),"${1}[redacted]"),
            (regex::Regex::new(r#"(?i)((?:password|passwd|secret|token|authorization|cookie|api[_-]?key)\s*[=:]\s*)(?:"[^"]*"|'[^']*'|[^\s,;]+)"#).unwrap(),"${1}[redacted]"),
            (regex::Regex::new(r"(?i)(https?://|postgres(?:ql)?://|rediss?://)[^/@\s]+@").unwrap(),"${1}[redacted]@"),
            (regex::Regex::new(r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+").unwrap(),"[redacted]"),
        ]
            },
        );
        for (pattern, replacement) in RULES.iter() {
            value = pattern.replace_all(&value, *replacement).into_owned();
        }
        value
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    }
}
#[derive(Default)]
struct Fields {
    message: String,
    module: Option<String>,
    metrics: Vec<String>,
}
fn metric(key: &str) -> bool {
    matches!(
        key,
        "time_ms"
            | "total_ms"
            | "load_ms"
            | "error_number"
            | "notice_number"
            | "input_tokens"
            | "output_tokens"
            | "pages"
            | "files"
    )
}
fn named(key: &str) -> bool {
    matches!(
        key,
        "stage" | "status" | "adapter" | "sqlstate" | "constraint" | "address"
    )
}
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{value:?}"),
            "module" => self.module = Some(format!("{value:?}").trim_matches('"').into()),
            key if named(key) || metric(key) => self.metrics.push(format!("{key}={value:?}")),
            _ => {}
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.into(),
            "module" => self.module = Some(value.into()),
            key if named(key) => self.metrics.push(format!("{key}={value}")),
            _ => {}
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        if metric(field.name()) {
            self.metrics.push(format!("{}={value}", field.name()));
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        if metric(field.name()) {
            self.metrics.push(format!("{}={value}", field.name()));
        }
    }
}
#[derive(Clone)]
pub struct LineFormat {
    pub config: Config,
    pub redactor: Redactor,
}
impl<S, N> FormatEvent<S, N> for LineFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        // Third-party diagnostics can include HTTP data. Only Cortex owns this log.
        if !meta.target().starts_with("cortex_api")
            || (*meta.level() == tracing::Level::DEBUG && !self.config.development)
            || *meta.level() == tracing::Level::TRACE
        {
            return Ok(());
        }
        let level = if *meta.level() == tracing::Level::WARN {
            "WARNING"
        } else {
            meta.level().as_str()
        };
        let mut fields = Fields::default();
        event.record(&mut fields);
        let module = self
            .redactor
            .clean(
                fields
                    .module
                    .as_deref()
                    .unwrap_or(meta.module_path().unwrap_or(APP)),
            )
            .replace(char::is_whitespace, "_");
        let message = if fields.metrics.is_empty() {
            fields.message
        } else {
            format!("{} {}", fields.message, fields.metrics.join(" "))
        };
        let mut line = format!(
            "{} {level} {APP} {module} {}",
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            self.redactor.clean(&message)
        );
        if line.len() > self.config.log_line_bytes {
            let mut end = self.config.log_line_bytes;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
        }
        writeln!(writer, "{line}")
    }
}
#[derive(Clone)]
pub struct LogHub {
    sender: broadcast::Sender<String>,
}
impl LogHub {
    pub fn new(config: &Config) -> Self {
        let (sender, _) = broadcast::channel(config.log_stream_capacity.max(1));
        Self { sender }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.sender.subscribe()
    }
}
pub struct LineWriter {
    hub: LogHub,
    buffer: Vec<u8>,
}
impl Write for LineWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        io::stdout().write_all(bytes)?;
        for &byte in bytes {
            if byte == b'\n' {
                if !self.buffer.is_empty() {
                    let _ = self
                        .hub
                        .sender
                        .send(String::from_utf8_lossy(&self.buffer).into_owned());
                }
                self.buffer.clear();
            } else {
                self.buffer.push(byte);
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogHub {
    type Writer = LineWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LineWriter {
            hub: self.clone(),
            buffer: vec![],
        }
    }
}
pub fn init(config: &Config) -> LogHub {
    let hub = LogHub::new(config);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(if config.development {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        })
        .event_format(LineFormat {
            config: config.clone(),
            redactor: Redactor::from_environment(),
        })
        .with_writer(hub.clone())
        .init();
    hub
}
// Static messages prevent caller text from entering these common functions.
pub fn debug(module: &'static str, message: &'static str) {
    tracing::debug!(module, message);
}
pub fn info(module: &'static str, message: &'static str) {
    tracing::info!(module, message);
}
pub fn warning(module: &'static str, message: &'static str) {
    tracing::warn!(module, message);
}
pub fn error(module: &'static str, message: &'static str) {
    tracing::error!(module, message);
}
