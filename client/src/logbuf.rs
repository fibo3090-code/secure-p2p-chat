//! In-process ring buffer of recent `tracing` events.
//!
//! The TUI's log overlay and the diagnostics bundle both need to show what the
//! app has been doing. That used to come from `egui_tracing::EventCollector` —
//! an egui crate pulled in purely for its collector, which kept the terminal UI
//! (and, transitively, the Tauri desktop bridge) depending on a GUI toolkit
//! that no longer ships. This is the same capability with no such dependency:
//! a `tracing_subscriber::Layer` that keeps the last [`LOG_CAPACITY`] events.
//!
//! Bounded on purpose: a long-running session must not grow a log buffer until
//! it becomes the app's largest allocation, and the overlay only ever shows the
//! recent tail anyway.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

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

    /// A snapshot of the retained events, oldest first.
    ///
    /// A poisoned lock is recovered from rather than propagated: losing the log
    /// overlay because some other thread panicked while logging would turn a
    /// diagnostic aid into a second failure.
    pub fn events(&self) -> Vec<LogEvent> {
        match self.inner.lock() {
            Ok(buf) => buf.iter().cloned().collect(),
            Err(poisoned) => poisoned.into_inner().iter().cloned().collect(),
        }
    }

    /// Number of retained events.
    pub fn len(&self) -> usize {
        match self.inner.lock() {
            Ok(buf) => buf.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop everything currently retained.
    pub fn clear(&self) {
        match self.inner.lock() {
            Ok(mut buf) => buf.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
        }
    }

    fn push(&self, event: LogEvent) {
        let mut buf = match self.inner.lock() {
            Ok(b) => b,
            Err(poisoned) => poisoned.into_inner(),
        };
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

    /// Events recorded through the real tracing pipeline must land in the
    /// buffer with their message intact — this is what the log overlay shows.
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

    /// An event carrying only structured fields still has to render as
    /// something a human can read, not an empty line.
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
