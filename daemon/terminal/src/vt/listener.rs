//! TermListener: alacritty_terminal::event::EventListener implementation,
//! plus channel envelopes (ReplyFrame, ControlFrame) and DropCounter.

// DropCounter is wired up by TermListener in Task 8-9; allow dead_code until then.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
