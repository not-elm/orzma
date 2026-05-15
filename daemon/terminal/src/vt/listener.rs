//! TermListener: alacritty_terminal::event::EventListener implementation,
//! plus channel envelopes (ReplyFrame, ControlFrame) and DropCounter.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Must-not-drop reply-required frames forwarded from `TermListener` into
/// the bridge task. The channel must be `mpsc::UnboundedSender` so that DA/DSR/
/// cursor-query replies never get dropped (which would silently break TUI
/// apps waiting for a response).
pub enum ReplyFrame {
    /// Bytes the Term emitted via `Event::PtyWrite` that must be written
    /// back to the PTY stdin (e.g., ANSI device-attribute responses).
    PtyWrite(Vec<u8>),
}

impl std::fmt::Debug for ReplyFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PtyWrite(b) => write!(f, "PtyWrite({} bytes)", b.len()),
        }
    }
}

/// Best-effort control frames forwarded from `TermListener`. The channel can
/// be bounded; `try_send` may drop. `DropCounter` aggregates drop counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlFrame {
    Title(String),
    ResetTitle,
    Bell,
    Clipboard {
        content: String,
        correlation_seq: Option<u32>,
    },
}

/// `alacritty_terminal::event::EventListener` implementation that bridges
/// Term-emitted events into bounded/unbounded mpsc channels feeding the
/// bridge task.
///
/// Channel split rationale (rust-daemon-design.md § 2.2):
///  - `reply_tx` (unbounded): reply-required events (`PtyWrite`). Dropping
///    these reproduces the capability-query backflow bug.
///  - `control_tx` (bounded): best-effort (`Title`, `ResetTitle`, `Bell`,
///    `ClipboardStore`). Drops are tracked via `DropCounter`.
pub struct TermListener {
    pub reply_tx: tokio::sync::mpsc::UnboundedSender<ReplyFrame>,
    pub control_tx: tokio::sync::mpsc::Sender<ControlFrame>,
    pub drop_counter: Arc<DropCounter>,
}

impl TermListener {
    fn send_control(&self, frame: ControlFrame, category: &'static str) {
        if self.control_tx.try_send(frame).is_err() && self.drop_counter.record(category) {
            tracing::warn!(
                category,
                total = self.drop_counter.total_count(),
                "control_tx full, dropped"
            );
        }
    }
}

impl alacritty_terminal::event::EventListener for TermListener {
    fn send_event(&self, event: alacritty_terminal::event::Event) {
        use alacritty_terminal::event::Event;

        match event {
            Event::PtyWrite(s) => {
                let _ = self.reply_tx.send(ReplyFrame::PtyWrite(s.into_bytes()));
            }

            Event::Title(s) => self.send_control(ControlFrame::Title(s), "title"),
            Event::ResetTitle => self.send_control(ControlFrame::ResetTitle, "reset_title"),
            Event::Bell => self.send_control(ControlFrame::Bell, "bell"),
            Event::ClipboardStore(_clip, content) => self.send_control(
                ControlFrame::Clipboard {
                    content,
                    correlation_seq: None,
                },
                "clipboard_store",
            ),

            // NOTE: TextAreaSizeRequest / ColorRequest / OSC 52 read are
            // currently no-ops; alacritty emits them when terminal apps query
            // state we don't track. Future opt-in would route through reply_tx.
            Event::TextAreaSizeRequest(_)
            | Event::ColorRequest(_, _)
            | Event::ClipboardLoad(_, _) => {}
            // NOTE: bridge task termination is handled via the PTY EOF path.
            Event::ChildExit(_) | Event::Exit => {}
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {}
        }
    }
}

/// Token-bucket rate-limited drop counter for bounded-channel `try_send`
/// failures. Prevents log spam while still surfacing aggregate counts.
#[derive(Debug)]
pub struct DropCounter {
    /// Total count of recorded drops across all categories.
    total: AtomicU64,
    /// Per-category state (token bucket + last refill time).
    buckets: Mutex<HashMap<&'static str, Bucket>>,
    tokens_per_window: u32,
    window: Duration,
}

#[derive(Debug)]
struct Bucket {
    tokens: u32,
    last_refill: Instant,
}

impl DropCounter {
    /// Default: 1 warn per second per category.
    pub fn new() -> Self {
        Self::with_tokens(1, Duration::from_secs(1))
    }

    /// Construct with explicit token-bucket parameters.
    pub fn with_tokens(tokens_per_window: u32, window: Duration) -> Self {
        Self {
            total: AtomicU64::new(0),
            buckets: Mutex::new(HashMap::new()),
            tokens_per_window,
            window,
        }
    }

    /// Total drop count (lifetime).
    pub fn total_count(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Record a drop event. Returns true if a warn-level log should be emitted
    /// (i.e., a token was available in the bucket for this category).
    pub fn record(&self, category: &'static str) -> bool {
        self.total.fetch_add(1, Ordering::Relaxed);
        self.should_warn(category)
    }

    /// Check if a warn log should fire for the given category.
    /// Takes `&self`; bucket state lives behind a `Mutex`.
    pub fn should_warn(&self, category: &'static str) -> bool {
        let mut buckets = self.buckets.lock().unwrap();
        let now = Instant::now();
        let bucket = buckets.entry(category).or_insert_with(|| Bucket {
            tokens: self.tokens_per_window,
            last_refill: now,
        });
        if now.duration_since(bucket.last_refill) >= self.window {
            bucket.tokens = self.tokens_per_window;
            bucket.last_refill = now;
        }
        if bucket.tokens > 0 {
            bucket.tokens -= 1;
            true
        } else {
            false
        }
    }
}

impl Default for DropCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod drop_counter_tests {
    use super::DropCounter;
    use std::time::Duration;

    #[test]
    fn first_record_logs() {
        let counter = DropCounter::new();
        counter.record("test");
        assert_eq!(counter.total_count(), 1);
    }

    #[test]
    fn multiple_records_increment() {
        let counter = DropCounter::new();
        for _ in 0..10 {
            counter.record("test");
        }
        assert_eq!(counter.total_count(), 10);
    }

    #[test]
    fn token_bucket_rate_limits() {
        let counter = DropCounter::with_tokens(2, Duration::from_millis(50));
        assert!(counter.should_warn("c"));
        assert!(counter.should_warn("c"));
        assert!(!counter.should_warn("c"));
    }
}

#[cfg(test)]
mod frame_envelope_tests {
    use super::*;

    #[test]
    fn control_frame_variants_construct() {
        let _ = ControlFrame::Title("hello".to_string());
        let _ = ControlFrame::ResetTitle;
        let _ = ControlFrame::Bell;
        let _ = ControlFrame::Clipboard {
            content: "x".to_string(),
            correlation_seq: Some(42),
        };
    }

    #[test]
    fn reply_frame_pty_write_holds_bytes() {
        let ReplyFrame::PtyWrite(bytes) = ReplyFrame::PtyWrite(vec![0x1b, b'[', b'?']);
        assert_eq!(bytes.len(), 3);
    }
}

#[cfg(test)]
mod listener_tests {
    use super::*;
    use alacritty_terminal::event::{Event, EventListener};
    use tokio::sync::mpsc;

    fn make_listener(
        reply_tx: mpsc::UnboundedSender<ReplyFrame>,
        control_tx: mpsc::Sender<ControlFrame>,
        drop_counter: std::sync::Arc<DropCounter>,
    ) -> TermListener {
        TermListener {
            reply_tx,
            control_tx,
            drop_counter,
        }
    }

    #[test]
    fn pty_write_event_is_forwarded() {
        let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, _control_rx) = mpsc::channel::<ControlFrame>(64);
        let drop_counter = std::sync::Arc::new(DropCounter::new());
        let listener = make_listener(reply_tx, control_tx, drop_counter);

        listener.send_event(Event::PtyWrite("\x1b[?6n".into()));
        let ReplyFrame::PtyWrite(bytes) = reply_rx.try_recv().expect("PtyWrite forwarded");
        assert_eq!(bytes, b"\x1b[?6n");
    }

    #[test]
    fn title_event_is_forwarded() {
        let (reply_tx, _reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, mut control_rx) = mpsc::channel::<ControlFrame>(64);
        let drop_counter = std::sync::Arc::new(DropCounter::new());
        let listener = make_listener(reply_tx, control_tx, drop_counter);

        listener.send_event(Event::Title("alpha".into()));
        let frame = control_rx.try_recv().expect("Title forwarded");
        assert_eq!(frame, ControlFrame::Title("alpha".to_string()));
    }

    #[test]
    fn bell_event_is_forwarded() {
        let (reply_tx, _reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, mut control_rx) = mpsc::channel::<ControlFrame>(64);
        let drop_counter = std::sync::Arc::new(DropCounter::new());
        let listener = make_listener(reply_tx, control_tx, drop_counter);

        listener.send_event(Event::Bell);
        assert_eq!(control_rx.try_recv().unwrap(), ControlFrame::Bell);
    }

    #[test]
    fn control_drop_records_to_counter() {
        let (reply_tx, _reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, _control_rx_held) = mpsc::channel::<ControlFrame>(1);
        let drop_counter = std::sync::Arc::new(DropCounter::new());
        let listener = make_listener(reply_tx, control_tx, drop_counter.clone());

        listener.send_event(Event::Title("first".into()));
        listener.send_event(Event::Title("second drops".into()));
        assert_eq!(drop_counter.total_count(), 1);
    }
}
