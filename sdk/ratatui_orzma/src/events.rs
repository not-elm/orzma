//! Per-handle inbound event queues: bounded rings the reader thread fills from
//! `op == "event"` lines and `WebviewHandle::read_events` drains by type.

use serde_json::Value;
use std::any::TypeId;
use std::collections::{HashMap, VecDeque};
use std::mem;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The default per-event ring capacity. Overflow drops the oldest payload.
const DEFAULT_CAP: usize = 1024;

/// Minimum interval between overflow warnings for a single saturated ring.
const WARN_EVERY: Duration = Duration::from_secs(5);

/// The ring name the overflow warning carries for focus changes.
const FOCUS_RING: &str = "focus_changed";

/// A change of webview focus the host reported for one placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusChange {
    /// The placement whose focus changed.
    pub instance: String,
    /// Whether the placement now holds webview focus.
    pub focused: bool,
}

/// A builder-time declaration binding a wire event name to a Rust type.
pub(crate) struct EventDecl {
    /// The wire event name the page sends via `window.orzma.emit`.
    pub(crate) name: String,
    /// `TypeId::of::<T>()` for the declared event type `T`.
    pub(crate) type_id: TypeId,
}

/// One bounded ring plus throttled-overflow bookkeeping.
#[derive(Debug)]
struct RingBuf<T> {
    buf: VecDeque<T>,
    dropped: u64,
    last_warn: Option<Instant>,
}

impl<T> Default for RingBuf<T> {
    fn default() -> Self {
        Self {
            buf: VecDeque::new(),
            dropped: 0,
            last_warn: None,
        }
    }
}

impl<T> RingBuf<T> {
    /// Appends `item`, dropping the oldest one first when `cap` is reached,
    /// with a warning naming `ring` at most once per `WARN_EVERY`.
    fn push(&mut self, item: T, cap: usize, ring: &str) {
        if self.buf.len() >= cap {
            self.buf.pop_front();
            self.dropped += 1;
            if self.last_warn.is_none_or(|t| t.elapsed() >= WARN_EVERY) {
                tracing::warn!(
                    ring,
                    dropped = self.dropped,
                    "inbound ring saturated; dropping oldest"
                );
                self.last_warn = Some(Instant::now());
            }
        }
        self.buf.push_back(item);
    }

    /// Takes every buffered item, oldest first.
    fn drain(&mut self) -> Vec<T> {
        Vec::from(mem::take(&mut self.buf))
    }
}

type Ring = Arc<Mutex<RingBuf<Value>>>;

/// The focus changes buffered for one handle, plus the placement last
/// reported focused.
#[derive(Default, Debug)]
struct FocusRing {
    changes: RingBuf<FocusChange>,
    focused: Option<String>,
}

/// The per-handle set of inbound event rings, declared at `register` and shared
/// between the reader thread (`by_name` ingest) and the `WebviewHandle`
/// (`by_type` drain). Each ring is shared by both maps via one `Arc`, so each
/// side reaches it in a single lookup. Both maps are frozen after construction;
/// only ring contents mutate, so the whole struct is shared behind one `Arc`
/// with no outer lock.
#[derive(Debug)]
pub(crate) struct EventQueues {
    by_name: HashMap<String, Ring>,
    by_type: HashMap<TypeId, Ring>,
    cap: usize,
    focus: Mutex<FocusRing>,
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
        ring.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(payload, self.cap, name);
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
        ring.lock().unwrap_or_else(|e| e.into_inner()).drain()
    }

    /// Buffers a `focus_changed` push for `instance`.
    pub fn ingest_focus(&self, instance: String, focused: bool) {
        let mut ring = self.focus.lock().unwrap_or_else(|e| e.into_inner());
        if focused {
            ring.focused = Some(instance.clone());
        } else if ring.focused.as_deref() == Some(instance.as_str()) {
            ring.focused = None;
        }
        ring.changes
            .push(FocusChange { instance, focused }, self.cap, FOCUS_RING);
    }

    /// Drains every buffered focus change, oldest first.
    pub fn drain_focus(&self) -> Vec<FocusChange> {
        self.focus
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .changes
            .drain()
    }

    /// Buffers `focused: false` for the placement last reported focused, if
    /// any, and forgets it.
    pub fn blur_all(&self) {
        let mut ring = self.focus.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(instance) = ring.focused.take() {
            ring.changes.push(
                FocusChange {
                    instance,
                    focused: false,
                },
                self.cap,
                FOCUS_RING,
            );
        }
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
            focus: Mutex::new(FocusRing::default()),
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

    /// Asserts that focus changes drain in arrival order and only once.
    ///
    /// Case: the user clicks a page and then presses the release shortcut
    /// between two iterations of the app's event loop.
    #[test]
    fn focus_changes_drain_in_order() {
        let q = EventQueues::from_decls(&[]);
        q.ingest_focus("i1".into(), true);
        q.ingest_focus("i1".into(), false);
        assert_eq!(
            q.drain_focus(),
            vec![
                FocusChange {
                    instance: "i1".into(),
                    focused: true,
                },
                FocusChange {
                    instance: "i1".into(),
                    focused: false,
                },
            ]
        );
        assert!(q.drain_focus().is_empty());
    }

    /// Asserts that `blur_all` reports `false` for the placement last
    /// reported focused, and only once.
    ///
    /// Case: focus moved from one placement to another, and then orzma's
    /// control socket drops while the second one holds focus, so the host's
    /// own `false` can no longer arrive.
    #[test]
    fn blur_all_reports_false_for_the_focused_placement_once() {
        let q = EventQueues::from_decls(&[]);
        q.ingest_focus("i1".into(), true);
        q.ingest_focus("i1".into(), false);
        q.ingest_focus("i2".into(), true);
        q.drain_focus();
        q.blur_all();
        assert_eq!(
            q.drain_focus(),
            vec![FocusChange {
                instance: "i2".into(),
                focused: false,
            }]
        );
        q.blur_all();
        assert!(q.drain_focus().is_empty());
    }

    /// Asserts that a full focus ring drops its oldest change.
    ///
    /// Case: an app stops reading focus changes while the user keeps
    /// clicking between pages.
    #[test]
    fn focus_overflow_drops_oldest() {
        let q = EventQueues::from_decls_with_cap(&[], 2);
        q.ingest_focus("a".into(), true);
        q.ingest_focus("b".into(), true);
        q.ingest_focus("c".into(), true);
        let drained: Vec<String> = q.drain_focus().into_iter().map(|c| c.instance).collect();
        assert_eq!(drained, vec!["b".to_owned(), "c".to_owned()]);
    }
}
