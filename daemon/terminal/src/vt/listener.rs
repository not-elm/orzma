//! TermListener: alacritty_terminal::event::EventListener implementation,
//! plus channel envelopes (ReplyFrame, ControlFrame) and DropCounter.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "DropCounter is wired up by TermListener in Task 8-9"
    )
)]

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
    #[expect(
        dead_code,
        reason = "constructed by TermListener in Task 9; only PtyWrite is tested here"
    )]
    TextAreaSizeRequest(Arc<dyn Fn(WindowSize) -> String + Send + Sync>),
    /// `Event::ColorRequest`: closure expects a palette `Rgb`.
    ///
    /// Note: alacritty 0.26's `Event::ColorRequest` uses
    /// `Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>` (not
    /// `Option<Rgb>`). The bridge must always have an `Rgb` to provide;
    /// the absent-color case is handled before invoking the closure.
    #[expect(
        dead_code,
        reason = "constructed by TermListener in Task 9; only PtyWrite is tested here"
    )]
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
