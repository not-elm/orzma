//! 256 KiB byte-budget ring of encoded MessagePack delta frames.

use bytes::Bytes;
use std::collections::VecDeque;

/// MessagePack-encoded delta with its seq for replay/identification.
#[derive(Debug, Clone)]
pub struct EncodedDelta {
    pub seq: u32,
    pub encoded: Bytes,
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

    /// Push a new encoded delta. Evicts oldest entries if budget exceeded.
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
        // else: single entry exceeds the budget; drop it silently
        // (Phase 2 で frame size cap 4 MiB に引っかかる前提なので発生しないはず)
    }

    /// Attempt to replay from `last_seq + 1` up to the latest seq.
    /// Returns `Some(deltas)` if the range is fully present (each consecutive
    /// seq exists in the ring). Returns `None` if there is any gap.
    pub fn replay(&self, last_seq: u32) -> Option<Vec<Bytes>> {
        let first_in_ring = self.deltas.front()?.seq;
        let target_first = last_seq.checked_add(1)?;
        if target_first < first_in_ring {
            return None; // gap: ring's oldest is past target_first
        }

        let mut out = Vec::new();
        let mut expected = target_first;
        for d in self.deltas.iter().skip_while(|d| d.seq < target_first) {
            if d.seq != expected {
                return None; // gap detected mid-range
            }
            out.push(d.encoded.clone());
            expected = expected.checked_add(1)?;
        }
        Some(out)
    }

    /// Latest seq in the ring (None if empty).
    pub fn latest_seq(&self) -> Option<u32> {
        self.deltas.back().map(|d| d.seq)
    }

    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
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
        assert!(r.is_empty());
    }

    #[test]
    fn push_and_replay_consecutive() {
        let mut r = FrameRing::new(1024);
        r.push(1, mk_bytes(10));
        r.push(2, mk_bytes(10));
        r.push(3, mk_bytes(10));
        let replay = r.replay(0).expect("range 1..=3 should be present");
        assert_eq!(replay.len(), 3);
        assert_eq!(r.latest_seq(), Some(3));
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
        // 各 entry 40 bytes、3 件で 120 → 最古 1 件が evicted
        r.push(1, mk_bytes(40));
        r.push(2, mk_bytes(40));
        r.push(3, mk_bytes(40));
        assert_eq!(r.len(), 2);
        // last_seq=0 で replay 要求 → ring に 2,3 しかないので gap = None
        assert!(r.replay(0).is_none());
        // last_seq=1 で replay → 2, 3 が並ぶ
        assert_eq!(r.replay(1).unwrap().len(), 2);
    }

    #[test]
    fn replay_at_latest_yields_empty_vec() {
        let mut r = FrameRing::new(1024);
        r.push(1, mk_bytes(10));
        let replay = r.replay(1).unwrap();
        assert!(replay.is_empty());
    }
}
