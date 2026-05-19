//! 256 KiB byte-budget ring of encoded wire frames, preserving WS opcode.

use bytes::Bytes;
use std::collections::VecDeque;

/// Wire-level broadcast envelope. Binary variants carry encoded MessagePack
/// (snapshot or delta) and are stored in `FrameRing` for replay. Text
/// variants carry JSON (hello / mode / error / clipboard) and are not
/// replayed — clients recover lost text sidecars via the next snapshot's
/// `modes` field.
#[derive(Clone, Debug)]
pub enum WireMessage {
    /// Encoded `FrameSnapshot` or `FrameDelta` payload with its sequence number.
    Binary {
        /// Monotonic frame sequence number.
        seq: u32,
        /// Map-keyed MessagePack payload.
        encoded: Bytes,
    },
    /// JSON-encoded text frame (`hello` / `mode` / `error` / `clipboard`).
    Text(String),
}

/// One stored frame in the ring. Preserves the WS opcode so replay routes
/// each entry to the correct WS frame type without re-deriving from payload.
#[derive(Debug, Clone)]
pub enum RingEntry {
    /// A binary-encoded MessagePack frame.
    Binary { seq: u32, encoded: Bytes },
    /// A mode-change JSON text frame.
    Mode { seq: u32, text: String },
    /// An error JSON text frame.
    Error { seq: u32, text: String },
}

impl RingEntry {
    fn seq(&self) -> u32 {
        match self {
            RingEntry::Binary { seq, .. }
            | RingEntry::Mode { seq, .. }
            | RingEntry::Error { seq, .. } => *seq,
        }
    }

    fn byte_cost(&self) -> usize {
        match self {
            RingEntry::Binary { encoded, .. } => encoded.len(),
            RingEntry::Mode { text, .. } | RingEntry::Error { text, .. } => text.len(),
        }
    }
}

/// FIFO ring with a byte-size budget. Oldest entries are evicted to make
/// room when `current_bytes + entry.len() > byte_budget`.
#[derive(Debug)]
pub struct FrameRing {
    entries: VecDeque<RingEntry>,
    byte_budget: usize,
    current_bytes: usize,
}

impl FrameRing {
    /// Construct with the given byte budget (e.g., 256 * 1024 for 256 KiB).
    pub fn new(byte_budget: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            byte_budget,
            current_bytes: 0,
        }
    }

    /// Pushes a binary-encoded MessagePack frame, evicting oldest entries if
    /// the budget would be exceeded.
    ///
    /// A frame whose own size exceeds the entire byte budget is dropped
    /// silently; Phase 2's 4 MiB frame size cap prevents this in practice.
    pub fn push_binary(&mut self, seq: u32, encoded: Bytes) {
        self.push_entry(RingEntry::Binary { seq, encoded });
    }

    /// Pushes a mode-change JSON text frame into the ring.
    pub fn push_mode(&mut self, seq: u32, text: String) {
        self.push_entry(RingEntry::Mode { seq, text });
    }

    /// Pushes an error JSON text frame into the ring.
    pub fn push_error(&mut self, seq: u32, text: String) {
        self.push_entry(RingEntry::Error { seq, text });
    }

    /// Returns the number of entries currently stored in the ring.
    pub(crate) fn entries_len(&self) -> usize {
        self.entries.len()
    }

    /// Replays consecutive entries from `last_seq + 1` up to the latest seq.
    ///
    /// Returns `Some(messages)` iff all seqs in the range are present
    /// (contiguity). Messages are returned in **insertion order** (the order
    /// they were pushed by the bridge) so the live `mode-before-binary` send
    /// order is preserved on replay. Returns `None` on any gap — caller must
    /// fall back to a full snapshot.
    ///
    /// Returns `Some(vec![])` when `last_seq` is already at or beyond the
    /// latest seq in the ring (client is up to date).
    pub fn replay(&self, last_seq: u32) -> Option<Vec<WireMessage>> {
        // Ring must be non-empty to establish contiguity.
        let latest_seq = self.entries.back()?.seq();
        // Client already has everything in the ring.
        if last_seq >= latest_seq {
            return Some(Vec::new());
        }
        let target_first = last_seq.wrapping_add(1);
        // If target_first is before the oldest entry, the ring has evicted the
        // needed frames — gap detected.
        let first_idx = self.entries.iter().position(|e| e.seq() >= target_first)?;
        let slice = self.entries.range(first_idx..);
        let mut expected = target_first;
        let mut out: Vec<WireMessage> = Vec::with_capacity(slice.len());
        for entry in slice {
            if entry.seq() != expected {
                return None;
            }
            expected = expected.wrapping_add(1);
            out.push(match entry {
                RingEntry::Binary { seq, encoded } => WireMessage::Binary {
                    seq: *seq,
                    encoded: encoded.clone(),
                },
                RingEntry::Mode { text, .. } | RingEntry::Error { text, .. } => {
                    WireMessage::Text(text.clone())
                }
            });
        }
        Some(out)
    }

    fn push_entry(&mut self, entry: RingEntry) {
        let cost = entry.byte_cost();
        while !self.entries.is_empty() && self.current_bytes + cost > self.byte_budget {
            if let Some(removed) = self.entries.pop_front() {
                self.current_bytes = self.current_bytes.saturating_sub(removed.byte_cost());
            }
        }
        if cost <= self.byte_budget {
            self.current_bytes += cost;
            self.entries.push_back(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_bytes(n: usize) -> Bytes {
        Bytes::from(vec![0u8; n])
    }

    #[test]
    fn empty_ring_returns_none_on_replay() {
        let r = FrameRing::new(1024);
        assert!(r.replay(0).is_none());
    }

    #[test]
    fn push_and_replay_consecutive() {
        let mut r = FrameRing::new(1024);
        r.push_binary(1, mk_bytes(10));
        r.push_binary(2, mk_bytes(10));
        r.push_binary(3, mk_bytes(10));
        let replay = r.replay(0).expect("range 1..=3 should be present");
        assert_eq!(replay.len(), 3);
    }

    #[test]
    fn replay_partial_range() {
        let mut r = FrameRing::new(1024);
        for seq in 1..=5 {
            r.push_binary(seq, mk_bytes(10));
        }
        let replay = r.replay(2).unwrap(); // expects seqs 3, 4, 5
        assert_eq!(replay.len(), 3);
    }

    #[test]
    fn eviction_when_over_budget() {
        let mut r = FrameRing::new(100);
        r.push_binary(1, mk_bytes(40));
        r.push_binary(2, mk_bytes(40));
        r.push_binary(3, mk_bytes(40));
        assert!(r.replay(0).is_none());
        assert_eq!(r.replay(1).unwrap().len(), 2);
    }

    #[test]
    fn replay_at_latest_yields_empty_vec() {
        let mut r = FrameRing::new(1024);
        r.push_binary(1, mk_bytes(10));
        let replay = r.replay(1).unwrap();
        assert!(replay.is_empty());
    }

    #[test]
    fn wire_message_binary_carries_seq_and_bytes() {
        let msg = WireMessage::Binary {
            seq: 7,
            encoded: Bytes::from_static(b"abc"),
        };
        match msg {
            WireMessage::Binary { seq, encoded } => {
                assert_eq!(seq, 7);
                assert_eq!(&encoded[..], b"abc");
            }
            WireMessage::Text(_) => panic!("wrong variant"),
        }
    }

    #[test]
    fn wire_message_text_carries_string() {
        let msg = WireMessage::Text("hello".to_string());
        match msg {
            WireMessage::Text(s) => assert_eq!(s, "hello"),
            WireMessage::Binary { .. } => panic!("wrong variant"),
        }
    }

    #[test]
    fn replay_returns_mixed_binary_and_mode_in_insertion_order() {
        let mut ring = FrameRing::new(64 * 1024);
        ring.push_binary(1, Bytes::from_static(b"binary-1"));
        ring.push_mode(2, "mode-2".to_string());
        ring.push_binary(3, Bytes::from_static(b"binary-3"));
        let out = ring.replay(0).expect("contiguous");
        assert_eq!(out.len(), 3);
        match &out[0] {
            WireMessage::Binary { seq, .. } => assert_eq!(*seq, 1),
            _ => panic!("expected Binary, got {:?}", &out[0]),
        }
        match &out[1] {
            WireMessage::Text(t) => assert_eq!(t, "mode-2"),
            _ => panic!("expected Text, got {:?}", &out[1]),
        }
        match &out[2] {
            WireMessage::Binary { seq, .. } => assert_eq!(*seq, 3),
            _ => panic!("expected Binary, got {:?}", &out[2]),
        }
    }

    #[test]
    fn replay_returns_none_when_last_seq_below_oldest() {
        let mut ring = FrameRing::new(64 * 1024);
        ring.push_binary(10, Bytes::from_static(b"x"));
        // last_seq=5 would need seq=6..10 contiguously; missing 6..9
        assert!(ring.replay(5).is_none());
    }

    #[test]
    fn push_error_is_replayed_as_text() {
        let mut ring = FrameRing::new(64 * 1024);
        ring.push_binary(1, Bytes::from_static(b"b"));
        ring.push_error(2, "{\"kind\":\"oversize_error\"}".to_string());
        let out = ring.replay(0).expect("contiguous");
        assert_eq!(out.len(), 2);
        match &out[1] {
            WireMessage::Text(t) => assert!(t.contains("oversize_error")),
            _ => panic!(),
        }
    }
}
