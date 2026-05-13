//! TermListener: alacritty_terminal::event::EventListener implementation,
//! plus channel envelopes (ReplyFrame, ControlFrame) and DropCounter.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use alacritty_terminal::vte::ansi::Rgb;

/// Pane window dimensions, used as the payload for `TextAreaSizeRequest` replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSize {
    pub num_lines: u16,
    pub num_cols: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

/// Must-not-drop reply-required frames forwarded from `TermListener` into
/// the bridge task. The channel must be `mpsc::UnboundedSender` so that DA/DSR/
/// cursor-query replies never get dropped (which would silently break TUI
/// apps waiting for a response).
pub enum ReplyFrame {
    /// Bytes the Term emitted via `Event::PtyWrite` that must be written
    /// back to the PTY stdin (e.g., ANSI device-attribute responses).
    PtyWrite(Vec<u8>),
    /// `Event::TextAreaSizeRequest`: caller closure expects a `WindowSize`
    /// reply; bridge fills it with the current pane dimensions.
    TextAreaSizeRequest(Arc<dyn Fn(WindowSize) -> String + Send + Sync>),
    /// `Event::ColorRequest`: closure expects a palette `Rgb`.
    ///
    /// Note: alacritty 0.26's `Event::ColorRequest` uses
    /// `Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>` (not
    /// `Option<Rgb>`). The bridge must always have an `Rgb` to provide;
    /// the absent-color case is handled before invoking the closure.
    ColorRequest(usize, Arc<dyn Fn(Rgb) -> String + Send + Sync>),
}

impl std::fmt::Debug for ReplyFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PtyWrite(b) => write!(f, "PtyWrite({} bytes)", b.len()),
            Self::TextAreaSizeRequest(_) => write!(f, "TextAreaSizeRequest(<fn>)"),
            Self::ColorRequest(idx, _) => write!(f, "ColorRequest({idx}, <fn>)"),
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
///  - `reply_tx` (unbounded): reply-required events (`PtyWrite`,
///    `TextAreaSizeRequest`, `ColorRequest`). Dropping these reproduces
///    the capability-query backflow bug.
///  - `control_tx` (bounded): best-effort (`Title`, `ResetTitle`, `Bell`,
///    `ClipboardStore`). Drops are tracked via `DropCounter`.
pub struct TermListener {
    pub reply_tx: tokio::sync::mpsc::UnboundedSender<ReplyFrame>,
    pub control_tx: tokio::sync::mpsc::Sender<ControlFrame>,
    pub drop_counter: Arc<DropCounter>,
}

impl alacritty_terminal::event::EventListener for TermListener {
    // Note: send_event is &self (sync, no Send/Sync supertrait).
    fn send_event(&self, event: alacritty_terminal::event::Event) {
        use alacritty_terminal::event::Event;

        match event {
            // ===== reply-required =====
            Event::PtyWrite(s) => {
                let _ = self.reply_tx.send(ReplyFrame::PtyWrite(s.into_bytes()));
            }
            Event::TextAreaSizeRequest(reply) => {
                // alacritty's closure takes alacritty's `WindowSize`; our
                // `ReplyFrame::TextAreaSizeRequest` carries one parameterized
                // by our local `WindowSize`. Both structs have identical
                // shape; wrap to bridge the type boundary.
                let wrapped: Arc<dyn Fn(WindowSize) -> String + Send + Sync> =
                    Arc::new(move |ours: WindowSize| {
                        reply(alacritty_terminal::event::WindowSize {
                            num_lines: ours.num_lines,
                            num_cols: ours.num_cols,
                            cell_width: ours.cell_width,
                            cell_height: ours.cell_height,
                        })
                    });
                let _ = self.reply_tx.send(ReplyFrame::TextAreaSizeRequest(wrapped));
            }
            Event::ColorRequest(idx, reply) => {
                let _ = self.reply_tx.send(ReplyFrame::ColorRequest(idx, reply));
            }

            // ===== best-effort =====
            Event::Title(s) => {
                if self.control_tx.try_send(ControlFrame::Title(s)).is_err()
                    && self.drop_counter.record("title")
                {
                    tracing::warn!(
                        category = "title",
                        total = self.drop_counter.total_count(),
                        "control_tx full, dropped"
                    );
                }
            }
            Event::ResetTitle => {
                if self.control_tx.try_send(ControlFrame::ResetTitle).is_err()
                    && self.drop_counter.record("reset_title")
                {
                    tracing::warn!(
                        category = "reset_title",
                        total = self.drop_counter.total_count(),
                        "control_tx full, dropped"
                    );
                }
            }
            Event::Bell => {
                if self.control_tx.try_send(ControlFrame::Bell).is_err()
                    && self.drop_counter.record("bell")
                {
                    tracing::warn!(
                        category = "bell",
                        total = self.drop_counter.total_count(),
                        "control_tx full, dropped"
                    );
                }
            }
            Event::ClipboardStore(_clip, content) => {
                let frame = ControlFrame::Clipboard {
                    content,
                    correlation_seq: None, // bridge task fills in latest input seq
                };
                if self.control_tx.try_send(frame).is_err()
                    && self.drop_counter.record("clipboard_store")
                {
                    tracing::warn!(
                        category = "clipboard_store",
                        total = self.drop_counter.total_count(),
                        "control_tx full, dropped"
                    );
                }
            }

            // ===== explicit ignores =====
            Event::ClipboardLoad(_clip, _reply) => {
                // OSC 52 read: default-ignored (security policy).
                // Future opt-in: route through reply_tx to ask the client.
            }
            Event::ChildExit(_) | Event::Exit => {
                // bridge task termination is handled via the PTY EOF path.
            }
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {
                // Phase 1/2 unused.
            }
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
        // 2 個 token があるので 2 回は warn 出力 OK、3 回目はスキップ
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
        let r = ReplyFrame::PtyWrite(vec![0x1b, b'[', b'?']);
        match r {
            ReplyFrame::PtyWrite(bytes) => assert_eq!(bytes.len(), 3),
            _ => panic!("variant mismatch"),
        }
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
        let frame = reply_rx.try_recv().expect("PtyWrite forwarded");
        match frame {
            ReplyFrame::PtyWrite(bytes) => assert_eq!(bytes, b"\x1b[?6n"),
            _ => panic!("wrong variant"),
        }
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
        // cap=1 control_tx, send Title twice, verify 1 drop is recorded.
        let (reply_tx, _reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, _control_rx_held) = mpsc::channel::<ControlFrame>(1);
        let drop_counter = std::sync::Arc::new(DropCounter::new());
        let listener = make_listener(reply_tx, control_tx, drop_counter.clone());

        listener.send_event(Event::Title("first".into()));
        listener.send_event(Event::Title("second drops".into()));
        // First fits (cap=1); second fails try_send -> drop_counter increments.
        assert_eq!(drop_counter.total_count(), 1);
    }
}
