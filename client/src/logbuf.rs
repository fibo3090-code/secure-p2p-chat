//! In-process ring buffer of recent `tracing` events, for the log overlay and
//! diagnostics bundles: a `tracing_subscriber::Layer` keeping the last
//! [`LOG_CAPACITY`] events, bounded so a long session cannot grow it forever.

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

/// Pulls the `message` field out of an event, falling back to the first field
/// so an event recorded only with structured fields still says something.
#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
    first_field: Option<String>,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            self.message = Some(rendered);
        } else if self.first_field.is_none() {
            self.first_field = Some(format!("{}={}", field.name(), rendered));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else if self.first_field.is_none() {
            self.first_field = Some(format!("{}={}", field.name(), value));
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
            message: visitor.message.or(visitor.first_field).unwrap_or_default(),
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
            tracing::warn!(peer = "alice", "structured too");
        });
        let events = buf.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].level, "INFO");
        assert_eq!(events[0].message, "hello from the pipeline");
        assert_eq!(events[1].level, "WARN");
        assert_eq!(events[1].message, "structured too");
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
