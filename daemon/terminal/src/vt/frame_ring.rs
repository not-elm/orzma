//! 256 KiB byte-budget ring of encoded MessagePack delta frames.

use bytes::Bytes;
use std::collections::VecDeque;

/// A MessagePack-encoded delta tagged with its sequence number.
#[derive(Debug, Clone)]
pub struct EncodedDelta {
    /// Monotonic frame sequence number.
    pub seq: u32,
    /// Encoded MessagePack payload.
    pub encoded: Bytes,
}

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

/// FIFO ring with a byte-size budget. Oldest entries are evicted to make
/// room when `current_bytes + entry.len() > byte_budget`.
#[derive(Debug)]
pub struct FrameRing {
    deltas: VecDeque<EncodedDelta>,
    byte_budget: usize,
    current_bytes: usize,
}

impl FrameRing {
    /// Construct with the given byte budget (e.g., 256 * 1024 for 256 KiB).
    pub fn new(byte_budget: usize) -> Self {
        Self {
            deltas: VecDeque::new(),
            byte_budget,
            current_bytes: 0,
        }
    }

    /// Pushes a new encoded delta, evicting oldest entries if the budget
    /// would be exceeded.
    ///
    /// A delta whose own size exceeds the entire byte budget is dropped
    /// silently; Phase 2's 4 MiB frame size cap prevents this in practice.
    pub fn push(&mut self, seq: u32, encoded: Bytes) {
        let size = encoded.len();
        while !self.deltas.is_empty() && self.current_bytes + size > self.byte_budget {
            if let Some(removed) = self.deltas.pop_front() {
                self.current_bytes -= removed.encoded.len();
            }
        }
        if size <= self.byte_budget {
            self.current_bytes += size;
            self.deltas.push_back(EncodedDelta { seq, encoded });
        }
    }

    /// Replays consecutive deltas from `last_seq + 1` up to the latest seq.
    ///
    /// Returns `Some(deltas)` only when every seq in the range is present;
    /// returns `None` on any gap (caller must fall back to a full snapshot).
    pub fn replay(&self, last_seq: u32) -> Option<Vec<Bytes>> {
        let first_in_ring = self.deltas.front()?.seq;
        let target_first = last_seq.checked_add(1)?;
        if target_first < first_in_ring {
            return None;
        }

        let mut out = Vec::new();
        let mut expected = target_first;
        for d in self.deltas.iter().skip_while(|d| d.seq < target_first) {
            if d.seq != expected {
                return None;
            }
            out.push(d.encoded.clone());
            expected = expected.checked_add(1)?;
        }
        Some(out)
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
        r.push(1, mk_bytes(10));
        r.push(2, mk_bytes(10));
        r.push(3, mk_bytes(10));
        let replay = r.replay(0).expect("range 1..=3 should be present");
        assert_eq!(replay.len(), 3);
    }

    #[test]
    fn replay_partial_range() {
        let mut r = FrameRing::new(1024);
        for seq in 1..=5 {
            r.push(seq, mk_bytes(10));
        }
        let replay = r.replay(2).unwrap(); // expects seqs 3, 4, 5
        assert_eq!(replay.len(), 3);
    }

    #[test]
    fn eviction_when_over_budget() {
        let mut r = FrameRing::new(100);
        r.push(1, mk_bytes(40));
        r.push(2, mk_bytes(40));
        r.push(3, mk_bytes(40));
        assert!(r.replay(0).is_none());
        assert_eq!(r.replay(1).unwrap().len(), 2);
    }

    #[test]
    fn replay_at_latest_yields_empty_vec() {
        let mut r = FrameRing::new(1024);
        r.push(1, mk_bytes(10));
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
}
