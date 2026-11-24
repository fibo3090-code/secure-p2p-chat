//! In-process ring buffer of recent `tracing` events, for the TUI's copy-logs
//! action (Ctrl+L) and diagnostics bundles: a `tracing_subscriber::Layer`
//! keeping the last [`LOG_CAPACITY`] events, bounded so a long session cannot
//! grow it forever.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// How many events are retained. Roughly a session's worth of `info`-level
/// activity, and a few hundred KiB at worst.
pub const LOG_CAPACITY: usize = 2_000;

/// One captured log line.
#[derive(Debug, Clone)]
pub struct LogEvent {
    pub time: DateTime<Utc>,
    pub level: &'static str,
    pub target: String,
    pub message: String,
}

/// A cloneable handle to the shared buffer. Install one clone as a tracing
/// layer and keep another to read from.
#[derive(Debug, Clone, Default)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<LogEvent>>>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(LOG_CAPACITY))),
        }
    }

    /// Recovers a poisoned lock: a thread that panicked while logging must not
    /// also take the log overlay down.
    fn buf(&self) -> MutexGuard<'_, VecDeque<LogEvent>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A snapshot of the retained events, oldest first.
    pub fn events(&self) -> Vec<LogEvent> {
        self.buf().iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.buf().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        self.buf().clear();
    }

    fn push(&self, event: LogEvent) {
        let mut buf = self.buf();
        if buf.len() == LOG_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(event);
    }
}

/// Renders an event the way `tracing_subscriber::fmt` does: the message,
/// then every other field as `key=value`. Keeping only the message dropped
/// the error, path or id that most structured calls carry (QUAL-06).
#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
    fields: String,
}

impl MessageVisitor {
    fn field(&mut self, name: &str, value: std::fmt::Arguments<'_>) {
        use std::fmt::Write;
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        let _ = write!(self.fields, "{name}={value}");
    }

    fn render(self) -> String {
        match (self.message, self.fields.is_empty()) {
            (Some(m), true) => m,
            (Some(m), false) => format!("{m} {}", self.fields),
            (None, _) => self.fields,
        }
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        } else {
            self.field(field.name(), format_args!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.field(field.name(), format_args!("{value}"));
        }
    }
}

impl<S: Subscriber> Layer<S> for LogBuffer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let meta = event.metadata();
        self.push(LogEvent {
            time: Utc::now(),
            level: meta.level().as_str(),
            target: meta.target().to_string(),
            message: visitor.render(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> LogEvent {
        LogEvent {
            time: Utc::now(),
            level: "INFO",
            target: "test".to_string(),
            message: format!("event {n}"),
        }
    }

    #[test]
    fn buffer_is_bounded_and_keeps_the_newest_events() {
        let buf = LogBuffer::new();
        for n in 0..(LOG_CAPACITY + 50) {
            buf.push(sample(n));
        }
        let events = buf.events();
        assert_eq!(events.len(), LOG_CAPACITY, "the buffer must stay bounded");
        assert_eq!(
            events.last().unwrap().message,
            format!("event {}", LOG_CAPACITY + 49),
            "the newest event must survive"
        );
        assert_eq!(
            events.first().unwrap().message,
            format!("event {}", 50),
            "the oldest events are the ones dropped"
        );
    }

    #[test]
    fn clones_share_one_buffer() {
        let buf = LogBuffer::new();
        let handle = buf.clone();
        handle.push(sample(1));
        assert_eq!(buf.len(), 1);
        buf.clear();
        assert!(handle.is_empty());
    }

    /// Events from the real tracing pipeline land with their message intact.
    #[test]
    fn captures_events_through_a_subscriber() {
        use tracing_subscriber::layer::SubscriberExt;
        let buf = LogBuffer::new();
        let subscriber = tracing_subscriber::registry().with(buf.clone());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("hello from the pipeline");
            // QUAL-06: the fields *are* the information — which file, what
            // error. They were dropped whenever a message was present, so
            // a copied log said "failed to persist party member" and nothing
            // about why.
            let e = std::io::Error::other("disk full");
            tracing::warn!(peer = "alice", error = %e, attempt = 2, "structured too");
            tracing::error!(path = "/x");
        });
        let events = buf.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].level, "INFO");
        assert_eq!(events[0].message, "hello from the pipeline");
        assert_eq!(events[1].level, "WARN");
        assert_eq!(
            events[1].message,
            "structured too peer=alice error=disk full attempt=2"
        );
        // No message: the fields alone.
        assert_eq!(events[2].message, "path=/x");
    }

    /// An event with only structured fields still renders as something.
    #[test]
    fn field_only_events_still_render() {
        use tracing_subscriber::layer::SubscriberExt;
        let buf = LogBuffer::new();
        let subscriber = tracing_subscriber::registry().with(buf.clone());
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(code = 42);
        });
        let events = buf.events();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].message.contains("code"),
            "field-only event rendered as {:?}",
            events[0].message
        );
    }
}
