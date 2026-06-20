//! Per-handle inbound event queues: bounded rings the reader thread fills from
//! `op == "event"` lines and `WebviewHandle::read_events` drains by type.

use serde_json::Value;
use std::any::TypeId;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The default per-event ring capacity. Overflow drops the oldest payload.
const DEFAULT_CAP: usize = 1024;

/// Minimum interval between overflow warnings for a single saturated ring.
const WARN_EVERY: Duration = Duration::from_secs(5);

/// A builder-time declaration binding a wire event name to a Rust type.
pub(crate) struct EventDecl {
    /// The wire event name the page sends via `window.ozma.emit`.
    pub(crate) name: String,
    /// `TypeId::of::<T>()` for the declared event type `T`.
    pub(crate) type_id: TypeId,
}

/// One bounded ring of raw payloads plus throttled-overflow bookkeeping.
#[derive(Default, Debug)]
struct RingBuf {
    buf: VecDeque<Value>,
    dropped: u64,
    last_warn: Option<Instant>,
}

type Ring = Arc<Mutex<RingBuf>>;

/// The per-handle set of inbound event rings, declared at `register` and shared
/// between the reader thread (`by_name` ingest) and the `WebviewHandle`
/// (`by_type` drain). Each ring is shared by both maps via one `Arc`, so each
/// side reaches it in a single lookup. Both maps are frozen after construction;
/// only ring contents mutate, so the whole struct is shared behind one `Arc`
/// with no outer lock.
#[derive(Default, Debug)]
pub(crate) struct EventQueues {
    by_name: HashMap<String, Ring>,
    by_type: HashMap<TypeId, Ring>,
    cap: usize,
}

/// Maps a registration handle to its `EventQueues`, the inbound-event peer of
/// the SDK's per-handle handler registry.
pub(crate) type EventRegistry = Arc<Mutex<HashMap<String, Arc<EventQueues>>>>;

impl EventQueues {
    /// Builds the rings for `decls` at the default capacity, inserting each ring
    /// into both lookup maps.
    pub(crate) fn from_decls(decls: &[EventDecl]) -> Self {
        Self::from_decls_with_cap(decls, DEFAULT_CAP)
    }

    /// Routes `payload` into the ring named `name`. When the ring is at
    /// capacity, drops the oldest payload first (with a per-ring throttled
    /// warning). Returns `false` when `name` was never declared.
    pub(crate) fn ingest(&self, name: &str, payload: Value) -> bool {
        let Some(ring) = self.by_name.get(name) else {
            return false;
        };
        let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.buf.len() >= self.cap {
            ring.buf.pop_front();
            ring.dropped += 1;
            if ring.last_warn.is_none_or(|t| t.elapsed() >= WARN_EVERY) {
                tracing::warn!(
                    event = name,
                    dropped = ring.dropped,
                    "inbound event ring saturated; dropping oldest"
                );
                ring.last_warn = Some(Instant::now());
            }
        }
        ring.buf.push_back(payload);
        true
    }

    /// Drains every buffered payload for the ring keyed by `type_id`, oldest
    /// first. Returns an empty `Vec` when the type was never declared. The ring
    /// lock is released before the caller deserializes, so a slow `from_value`
    /// never blocks the reader thread's ingest.
    pub(crate) fn drain_type(&self, type_id: TypeId) -> Vec<Value> {
        let Some(ring) = self.by_type.get(&type_id) else {
            return Vec::new();
        };
        let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
        Vec::from(std::mem::take(&mut ring.buf))
    }

    fn from_decls_with_cap(decls: &[EventDecl], cap: usize) -> Self {
        let mut by_name = HashMap::new();
        let mut by_type = HashMap::new();
        for decl in decls {
            let ring: Ring = Arc::new(Mutex::new(RingBuf::default()));
            by_name.insert(decl.name.clone(), ring.clone());
            by_type.insert(decl.type_id, ring);
        }
        Self {
            by_name,
            by_type,
            cap,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct A;
    struct B;

    fn decls() -> Vec<EventDecl> {
        vec![
            EventDecl {
                name: "a".into(),
                type_id: TypeId::of::<A>(),
            },
            EventDecl {
                name: "b".into(),
                type_id: TypeId::of::<B>(),
            },
        ]
    }

    #[test]
    fn ingest_then_drain_by_type_is_fifo() {
        let q = EventQueues::from_decls(&decls());
        assert!(q.ingest("a", json!(1)));
        assert!(q.ingest("a", json!(2)));
        let drained = q.drain_type(TypeId::of::<A>());
        assert_eq!(drained, vec![json!(1), json!(2)]);
        // A second drain is empty; the ring was consumed.
        assert!(q.drain_type(TypeId::of::<A>()).is_empty());
    }

    #[test]
    fn drain_is_isolated_per_type() {
        let q = EventQueues::from_decls(&decls());
        q.ingest("a", json!("x"));
        q.ingest("b", json!("y"));
        assert_eq!(q.drain_type(TypeId::of::<A>()), vec![json!("x")]);
        assert_eq!(q.drain_type(TypeId::of::<B>()), vec![json!("y")]);
    }

    #[test]
    fn ingest_for_undeclared_name_returns_false() {
        let q = EventQueues::from_decls(&decls());
        assert!(!q.ingest("missing", json!(1)));
    }

    #[test]
    fn drain_for_undeclared_type_is_empty() {
        struct C;
        let q = EventQueues::from_decls(&decls());
        assert!(q.drain_type(TypeId::of::<C>()).is_empty());
    }

    #[test]
    fn overflow_drops_oldest_and_keeps_cap() {
        let q = EventQueues::from_decls_with_cap(&decls(), 2);
        q.ingest("a", json!(1));
        q.ingest("a", json!(2));
        q.ingest("a", json!(3)); // evicts 1
        let drained = q.drain_type(TypeId::of::<A>());
        assert_eq!(drained, vec![json!(2), json!(3)]);
    }
}
