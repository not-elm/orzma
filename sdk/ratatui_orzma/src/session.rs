//! The Orzma session: socket connection, reader thread, flush.

use crate::error::{OrzmaError, OrzmaResult};
use crate::escape::{clamp_dims, cursor_to, mount, unmount, valid_instance};
use crate::events::{EventQueues, EventRegistry};
use crate::handler::BoxedHandler;
use crate::protocol::{
    ClientMsg, HandleId, IncomingCall, IncomingEvent, RegisterKind, ServerReply,
};
use crate::uds::UnixStream;
use crate::webview::{SharedWriter, Webview, WebviewHandle, WebviewInstance};
use crossbeam_channel::{Receiver, Sender, bounded};
use ratatui::layout::Rect;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

/// How long a blocking control-socket request waits for its reply before
/// giving up on it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// One placement's requested position this frame.
#[derive(Debug, Clone)]
pub(crate) struct Placement {
    pub instance: String,
    pub area: Rect,
}

/// The per-frame collector handed to the [`crate::WebviewWidget`] as its state.
///
/// Everything it holds is keyed by placement instance, so two placements of one
/// registration are two independent entries rather than one shared key.
#[derive(Debug, Default)]
pub struct FramePlacements {
    placements: Vec<Placement>,
    focused: Option<String>,
    pub(crate) pending_compositing: HashMap<String, bool>,
}

impl FramePlacements {
    pub(crate) fn record(&mut self, instance: String, area: Rect) {
        self.placements.push(Placement { instance, area });
    }

    /// Marks `instance` focused for this frame. Last writer wins; a debug build
    /// trips an assertion if more than one widget claims focus in a single frame
    /// (the app must focus at most one placement at a time).
    pub(crate) fn set_focused(&mut self, instance: String) {
        debug_assert!(
            self.focused.is_none(),
            "multiple webviews marked focused in one frame (last wins): had {:?}, now {instance:?}",
            self.focused
        );
        self.focused = Some(instance);
    }

    /// Removes and returns the buffered compositing state for `instance`, if any.
    pub(crate) fn take_compositing(&mut self, instance: &str) -> Option<bool> {
        self.pending_compositing.remove(instance)
    }

    #[cfg(test)]
    pub(crate) fn placements_for_test(&self) -> &[Placement] {
        &self.placements
    }

    #[cfg(test)]
    pub(crate) fn focused_for_test(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn pending_compositing_for_test(&self) -> &HashMap<String, bool> {
        &self.pending_compositing
    }
}

/// Last-emitted geometry per instance, for diff-driven flush.
#[derive(Debug, Default)]
pub(crate) struct FlushState {
    #[cfg(test)]
    pub last: HashMap<String, Rect>,
    #[cfg(not(test))]
    last: HashMap<String, Rect>,
    last_focused: Option<String>,
}

impl FlushState {
    /// Emits this frame's geometry and, when focus changed since the last
    /// frame, the control-plane focus op.
    ///
    /// On Unix the geometry rides the PTY as APC verbs (`out`) and only the
    /// focus op takes the socket; on Windows ConPTY drops APC, so the
    /// geometry takes the socket too, as `mount` / `unmount` ops.
    pub fn emit_frame(
        &mut self,
        out: &mut impl Write,
        socket: &SharedWriter,
        frame: &FramePlacements,
    ) -> OrzmaResult<()> {
        if cfg!(windows) {
            // NOTE: one lock for the whole frame — the writer is shared with
            // the reader thread and every WebviewHandle::emit, and geometry
            // plus focus must not interleave with a concurrent emit line.
            let mut w = socket.lock()?;
            flush_placements_over_socket(&mut *w, self, &frame.placements)?;
            if self.last_focused != frame.focused {
                flush_focus(&mut *w, &mut self.last_focused, &frame.focused)?;
            }
            return Ok(());
        }
        self.emit_placements(out, frame)?;
        // NOTE: only take the writer lock (shared with the reader thread and
        // every WebviewHandle::emit) when focus actually changed; this runs every
        // render frame and the unchanged path must not contend the lock.
        if self.last_focused == frame.focused {
            return Ok(());
        }
        let mut w = socket.lock()?;
        flush_focus(&mut *w, &mut self.last_focused, &frame.focused)
    }

    /// Emits this frame's geometry to `out` alone, leaving the focus op unsent.
    ///
    /// This is the flush to use while the control socket is down. On Unix
    /// geometry rides the PTY, which outlives the socket, and there is nothing
    /// at the other end of the socket to receive a focus op — attempting one
    /// would only fail the whole draw. On Windows geometry needs the socket
    /// too, so nothing is emitted until the reconnect, after which
    /// [`Self::reset`] re-asserts every placement.
    pub fn emit_placements(
        &mut self,
        out: &mut impl Write,
        frame: &FramePlacements,
    ) -> OrzmaResult<()> {
        if cfg!(windows) {
            return Ok(());
        }
        flush_placements(out, self, &frame.placements)
    }

    /// Drops the record of what was last emitted, so the next flush re-asserts
    /// this frame's geometry and focus from scratch.
    ///
    /// Called when the connection the record described has gone: the host
    /// forgets every mount when the socket drops, so diffing against it would
    /// be diffing against state that no longer exists anywhere. Re-registering
    /// also mints fresh instance ids, which the diff already treats as new
    /// placements; what clearing adds is that the vanished-instance pass then
    /// emits no unmount for an id the host has already dropped.
    pub fn reset(&mut self) {
        self.last.clear();
        self.last_focused = None;
    }
}

type HandlerRegistry = Arc<Mutex<HashMap<String, Arc<HashMap<String, BoxedHandler>>>>>;
type PendingReplies = Arc<Mutex<VecDeque<Pending>>>;

/// One in-flight request awaiting its op-less reply.
///
/// An entry outlives its waiter's [`REPLY_TIMEOUT`] on purpose: a late reply
/// still corresponds to this entry by position, so dropping the entry would
/// hand that reply to the next waiter in line.
enum Pending {
    /// A `register`: the oneshot carries the minted `(handle, instance)` pair,
    /// and the handlers and event queues are installed under that handle
    /// before the caller is woken.
    ///
    /// The handle stays a [`HandleId`] across the channel. The two ids are
    /// otherwise distinguished only by tuple position, and a consumer that
    /// swapped them would mount a handle — which the host can never resolve.
    Register {
        reply: Sender<OrzmaResult<(HandleId, String)>>,
        handlers: Arc<HashMap<String, BoxedHandler>>,
        events: Arc<EventQueues>,
    },
    /// A `new_instance`: the oneshot carries the minted instance.
    NewInstance { reply: Sender<OrzmaResult<String>> },
}

type PendingCompositing = Arc<Mutex<HashMap<String, bool>>>;

/// A saved webview registration for replay on reconnect, holding every id slot
/// a reconnect has to refill: the handle, the default instance, and one slot
/// per instance minted by [`WebviewHandle::new_instance`].
struct Registration {
    kind: RegisterKind,
    handle_slot: Arc<Mutex<HandleId>>,
    instance_slot: Arc<Mutex<String>>,
    extra_instances: Vec<Arc<Mutex<String>>>,
    handlers: Arc<HashMap<String, BoxedHandler>>,
    events: Arc<EventQueues>,
}

impl Registration {
    /// The handle this registration currently answers to.
    fn handle_id(&self) -> HandleId {
        self.handle_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// The minimal shared state passed to [`OrzmaBackend`] for reconnect signalling.
pub(crate) struct ReconnectHandle {
    pub(crate) disconnected: Arc<AtomicBool>,
    pub(crate) generation: Arc<AtomicU64>,
    pub(crate) reconnect_tx: Sender<()>,
}

/// The request and replay plumbing a [`WebviewHandle`] needs in order to mint
/// on its own registration: the pending-reply queue a request is tracked
/// through, and the registrations a reconnect replays.
///
/// A handle reaches this through a [`std::sync::Weak`], so holding one never
/// keeps a session alive. The fields stay behind their own `Arc`s because the
/// reader and reconnect threads capture those directly — dropping the [`Orzma`]
/// therefore drops this core (failing every later `upgrade`) while leaving the
/// threads their own view of the same state.
pub(crate) struct SessionCore {
    pending: PendingReplies,
    registrations: Arc<Mutex<Vec<Registration>>>,
}

impl SessionCore {
    /// Mints an additional placement on `handle`, writing the request through
    /// `writer` and blocking until the control plane answers it.
    pub(crate) fn mint_instance(
        &self,
        writer: &SharedWriter,
        handle: &WebviewHandle,
    ) -> OrzmaResult<WebviewInstance> {
        // NOTE: the saved registrations stay locked from before the request goes
        // out until after the minted slot is recorded. A reconnect replays them
        // under this same lock, so releasing it any earlier would let a replay
        // complete in between and leave this slot holding an instance minted on
        // the connection that just died — one the app can never mount.
        let mut regs = self.registrations.lock().unwrap_or_else(|e| e.into_inner());
        let handle_id = handle.handle_id();
        let (tx, rx) = bounded(1);
        let line = serde_json::to_string(&ClientMsg::NewInstance {
            handle: handle_id.clone(),
        })?;
        send_request(
            writer,
            &self.pending,
            Pending::NewInstance { reply: tx },
            &line,
        )?;
        let slot = Arc::new(Mutex::new(await_reply(&rx)?));
        match regs
            .iter_mut()
            .find(|r| handle.shares_handle_slot(&r.handle_slot))
        {
            Some(reg) => reg.extra_instances.push(slot.clone()),
            None => tracing::debug!(
                handle = %handle_id,
                "minted an instance for a handle with no saved registration; it will not survive a reconnect"
            ),
        }
        Ok(WebviewInstance::new_shared(slot, writer.clone()))
    }
}

/// An orzma session: owns the control-socket connection and reader thread.
pub struct Orzma {
    writer: SharedWriter,
    core: Arc<SessionCore>,
    frame: Arc<Mutex<FramePlacements>>,
    pending_compositing: PendingCompositing,
    disconnected: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    reconnect_tx: crossbeam_channel::Sender<()>,
}

impl Orzma {
    /// Connects to the orzma control socket, performs the `hello` handshake, and
    /// spawns the background reader thread.
    ///
    /// The socket path comes from the inherited `$ORZMA_SOCK`, and the identity
    /// sent in the handshake from `$ORZMA_TOKEN`; orzma injects both into every
    /// surface it spawns. An inherited `$ORZMA_SOCK` can be a stale snapshot from
    /// an orzma that has since exited, so an unreachable socket surfaces as
    /// [`OrzmaError::SocketUnavailable`] rather than a bare IO error.
    pub fn connect() -> OrzmaResult<Self> {
        let sock = resolve_orzma_sock().ok_or(OrzmaError::NotInPane("ORZMA_SOCK"))?;
        let token = resolve_orzma_token().ok_or(OrzmaError::NotInPane("ORZMA_TOKEN"))?;
        let stream = connect_sock(&sock)?;
        let writer: SharedWriter = Arc::new(Mutex::new(stream.try_clone()?));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingReplies = Arc::new(Mutex::new(VecDeque::new()));
        let pending_compositing: PendingCompositing = Arc::new(Mutex::new(HashMap::new()));
        let disconnected: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let generation: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
        let registrations: Arc<Mutex<Vec<Registration>>> = Arc::new(Mutex::new(Vec::new()));
        let events: EventRegistry = Arc::new(Mutex::new(HashMap::new()));
        let (reconnect_tx, reconnect_rx) = crossbeam_channel::bounded::<()>(1);

        {
            let line = serde_json::to_string(&ClientMsg::Hello {
                token: token.clone(),
            })?;
            let mut w = writer.lock()?;
            writeln!(w, "{line}")?;
            w.flush()?;
        }

        spawn_reader(
            stream,
            writer.clone(),
            handlers.clone(),
            pending.clone(),
            pending_compositing.clone(),
            events.clone(),
            disconnected.clone(),
        );

        {
            let writer2 = writer.clone();
            let handlers2 = handlers.clone();
            let pending2 = pending.clone();
            let pending_compositing2 = pending_compositing.clone();
            let disconnected2 = disconnected.clone();
            let generation2 = generation.clone();
            let registrations2 = registrations.clone();
            let events2 = events.clone();
            let token2 = token.clone();
            thread::spawn(move || {
                while let Ok(()) = reconnect_rx.recv() {
                    attempt_reconnect(
                        &writer2,
                        &handlers2,
                        &pending2,
                        &pending_compositing2,
                        &disconnected2,
                        &generation2,
                        &registrations2,
                        &events2,
                        &token2,
                    );
                }
            });
        }

        Ok(Self {
            writer,
            core: Arc::new(SessionCore {
                pending,
                registrations,
            }),
            frame: Arc::new(Mutex::new(FramePlacements::default())),
            pending_compositing,
            disconnected,
            generation,
            reconnect_tx,
        })
    }

    /// Registers a webview, blocking until the control plane mints its handle
    /// and that registration's first placement instance.
    ///
    /// Blocks on a control-socket round trip, so call it while setting the app
    /// up — never from inside the draw loop.
    pub fn register(&self, webview: Webview) -> OrzmaResult<WebviewHandle> {
        let Webview {
            kind,
            handlers,
            event_decls,
        } = webview;
        let handlers = Arc::new(handlers);
        let events = Arc::new(EventQueues::from_decls(&event_decls));
        let (tx, rx) = bounded(1);
        let line = serde_json::to_string(&ClientMsg::Register(kind.clone()))?;
        send_request(
            &self.writer,
            &self.core.pending,
            Pending::Register {
                reply: tx,
                handlers: handlers.clone(),
                events: events.clone(),
            },
            &line,
        )?;
        let (handle, instance) = await_reply(&rx)?;
        let handle_slot = Arc::new(Mutex::new(handle));
        let instance_slot = Arc::new(Mutex::new(instance));
        // NOTE: save through a poisoned lock rather than skipping the push. A
        // returned handle whose registration was never saved mints placements
        // no reconnect can replay, and reports nothing when it does.
        self.core
            .registrations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Registration {
                kind,
                handle_slot: handle_slot.clone(),
                instance_slot: instance_slot.clone(),
                extra_instances: Vec::new(),
                handlers: handlers.clone(),
                events: events.clone(),
            });
        Ok(WebviewHandle::new_shared(
            handle_slot,
            instance_slot,
            events,
            self.writer.clone(),
            Arc::downgrade(&self.core),
        ))
    }

    /// Locks and clears the per-frame placement collector for `render_stateful_widget`.
    ///
    /// The returned guard derefs to [`FramePlacements`]; pass `&mut *orzma.frame()`
    /// as the widget state. Let it drop at the end of the `terminal.draw` closure
    /// so the [`crate::OrzmaBackend`] can read the frame during that draw's flush.
    /// Drains any pending compositing notifications from the reader thread into
    /// the frame so widget code can read them via `FramePlacements::take_compositing`.
    pub fn frame(&self) -> MutexGuard<'_, FramePlacements> {
        let mut frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
        frame.placements.clear();
        frame.focused = None;
        frame.pending_compositing = std::mem::take(
            &mut *self
                .pending_compositing
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        frame
    }

    pub(crate) fn frame_handle(&self) -> Arc<Mutex<FramePlacements>> {
        self.frame.clone()
    }

    pub(crate) fn writer_handle(&self) -> SharedWriter {
        self.writer.clone()
    }

    /// Returns a handle to the reconnect machinery for use by [`crate::OrzmaBackend`].
    pub(crate) fn reconnect_handle(&self) -> ReconnectHandle {
        ReconnectHandle {
            disconnected: self.disconnected.clone(),
            generation: self.generation.clone(),
            reconnect_tx: self.reconnect_tx.clone(),
        }
    }
}

/// The control-socket path inherited from the surface's environment. `None` when
/// `$ORZMA_SOCK` is unset or empty — the process is not inside an orzma surface.
fn resolve_orzma_sock() -> Option<String> {
    std::env::var("ORZMA_SOCK").ok().filter(|s| !s.is_empty())
}

/// The identity sent in the `hello` handshake, read from the per-surface
/// `$ORZMA_TOKEN`. `None` when it is unset or empty.
fn resolve_orzma_token() -> Option<String> {
    std::env::var("ORZMA_TOKEN").ok().filter(|t| !t.is_empty())
}

/// Connects to `sock`, distinguishing a stale socket from a genuine IO failure.
///
/// `NotFound` (the socket file is gone) and `ConnectionRefused` (the file exists
/// but its orzma has exited) mean the inherited `$ORZMA_SOCK` is stale, and are
/// reported as [`OrzmaError::SocketUnavailable`]. Any other IO error is surfaced
/// as [`OrzmaError::Io`].
fn connect_sock(sock: &str) -> OrzmaResult<UnixStream> {
    match UnixStream::connect(sock) {
        Ok(stream) => Ok(stream),
        Err(cause) if is_stale_socket_error(cause.kind()) => Err(OrzmaError::SocketUnavailable {
            path: sock.to_owned(),
            cause,
        }),
        Err(e) => Err(OrzmaError::Io(e)),
    }
}

/// Whether a connect failure means the socket's orzma is gone rather than a
/// genuine IO fault.
///
/// Windows AF_UNIX reports a socket whose parent directory has been removed as
/// `WSAENETDOWN` (`NetworkDown`), and a missing file or dead listener as
/// `ConnectionRefused`; Unix reports `NotFound` / `ConnectionRefused`.
fn is_stale_socket_error(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::NotFound | ErrorKind::ConnectionRefused)
        || (cfg!(windows) && kind == ErrorKind::NetworkDown)
}

/// One geometry change a flush must announce, before it is spelled as an
/// APC verb or a socket op.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PlacementVerb {
    /// Mount (or re-mount) `instance` at the 0-based cell (`row`, `col`)
    /// with the clamped size.
    Mount {
        instance: String,
        row: u16,
        col: u16,
        rows: u16,
        cols: u16,
    },
    /// Unmount `instance`, which was drawn last flush but not this one.
    Unmount { instance: String },
}

impl PlacementVerb {
    /// Diffs `placements` against `state.last`: a placement whose rect is
    /// new or moved yields a `Mount`, an instance seen last flush but absent
    /// now yields an `Unmount`. Returns the verbs and the map that becomes
    /// `state.last` once they are written.
    ///
    /// A placement whose rect is degenerate, or whose id is not a minted
    /// instance, is skipped and logged rather than propagated as an error: a
    /// single bad placement must not abort the flush, which would also
    /// desync `state` for every placement behind it. Such an id can never
    /// mount, so the log is the only trace the caller would otherwise get.
    fn diff(state: &FlushState, placements: &[Placement]) -> (Vec<Self>, HashMap<String, Rect>) {
        let mut current: HashMap<String, Rect> = HashMap::new();
        let mut verbs = Vec::new();
        for p in placements {
            if p.area.width == 0 || p.area.height == 0 || !valid_instance(&p.instance) {
                tracing::debug!(
                    instance = %p.instance,
                    "skipping a placement with a degenerate area or an unusable instance id"
                );
                continue;
            }
            let (rows, cols) = clamp_dims(p.area.height, p.area.width);
            let key = Rect {
                x: p.area.x,
                y: p.area.y,
                width: cols,
                height: rows,
            };
            current.insert(p.instance.clone(), key);
            if state.last.get(&p.instance) != Some(&key) {
                verbs.push(Self::Mount {
                    instance: p.instance.clone(),
                    row: p.area.y,
                    col: p.area.x,
                    rows,
                    cols,
                });
            }
        }
        for instance in state.last.keys() {
            if !current.contains_key(instance) {
                verbs.push(Self::Unmount {
                    instance: instance.clone(),
                });
            }
        }
        (verbs, current)
    }
}

/// Emits CUP + mount for new and moved placements, and unmount for instances
/// that vanished, updating `state` to the new frame: the APC spelling of
/// [`PlacementVerb::diff`], for hosts whose PTY passes APC through.
fn flush_placements(
    out: &mut impl Write,
    state: &mut FlushState,
    placements: &[Placement],
) -> OrzmaResult<()> {
    let (verbs, current) = PlacementVerb::diff(state, placements);
    for verb in verbs {
        match verb {
            PlacementVerb::Mount {
                instance,
                row,
                col,
                rows,
                cols,
            } => {
                let seq = mount(&instance, rows, cols)?;
                write!(out, "{}{}", cursor_to(row, col), seq)?;
            }
            PlacementVerb::Unmount { instance } => write!(out, "{}", unmount(&instance))?,
        }
    }
    out.flush()?;
    state.last = current;
    Ok(())
}

/// Emits the socket `mount` op for new and moved placements and the socket
/// `unmount` op for instances that vanished, updating `state` to the new
/// frame: the control-socket spelling of [`PlacementVerb::diff`], for hosts
/// whose PTY drops APC (ConPTY on Windows). One NDJSON line per verb.
fn flush_placements_over_socket(
    socket: &mut impl Write,
    state: &mut FlushState,
    placements: &[Placement],
) -> OrzmaResult<()> {
    let (verbs, current) = PlacementVerb::diff(state, placements);
    for verb in verbs {
        let msg = match verb {
            PlacementVerb::Mount {
                instance,
                row,
                col,
                rows,
                cols,
            } => ClientMsg::Mount {
                instance,
                row,
                col,
                rows,
                cols,
            },
            PlacementVerb::Unmount { instance } => ClientMsg::Unmount { instance },
        };
        serde_json::to_writer(&mut *socket, &msg)?;
        socket.write_all(b"\n")?;
    }
    socket.flush()?;
    state.last = current;
    Ok(())
}

/// Emits the control-plane focus op (`ClientMsg::Focus`) when the focused
/// instance changed from the last flush. `Some(i)` focuses placement `i`;
/// `None` blurs. No write when unchanged (diff-driven, like geometry in
/// `flush_placements`).
fn flush_focus(
    out: &mut impl Write,
    last_focused: &mut Option<String>,
    focused: &Option<String>,
) -> OrzmaResult<()> {
    if last_focused == focused {
        return Ok(());
    }
    let line = serde_json::to_string(&ClientMsg::Focus {
        instance: focused.clone(),
    })?;
    writeln!(out, "{line}")?;
    out.flush()?;
    *last_focused = focused.clone();
    Ok(())
}

fn spawn_reader(
    stream: UnixStream,
    writer: SharedWriter,
    handlers: HandlerRegistry,
    pending: PendingReplies,
    pending_compositing: PendingCompositing,
    events: EventRegistry,
    disconnected: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed = serde_json::from_str::<serde_json::Value>(trimmed).ok();
            let op = parsed.as_ref().and_then(|v| v["op"].as_str()).unwrap_or("");
            if op == "call" {
                if let Ok(call) = serde_json::from_str::<IncomingCall>(trimmed) {
                    dispatch_call(&writer, &handlers, call);
                }
            } else if op == "compositing" {
                if let Some(v) = parsed.as_ref()
                    && let Some(instance) = v["instance"].as_str()
                    && let Some(active) = v["active"].as_bool()
                    && let Ok(mut map) = pending_compositing.lock()
                {
                    map.insert(instance.to_owned(), active);
                }
            } else if op == "event" {
                if let Ok(ev) = serde_json::from_str::<IncomingEvent>(trimmed) {
                    match events
                        .lock()
                        .ok()
                        .and_then(|map| map.get(&ev.handle).cloned())
                    {
                        Some(queues) => {
                            if !queues.ingest(&ev.event, ev.payload) {
                                tracing::debug!(
                                    handle = ev.handle,
                                    event = ev.event,
                                    "inbound event for an undeclared name dropped"
                                );
                            }
                        }
                        None => tracing::debug!(
                            handle = ev.handle,
                            event = ev.event,
                            "inbound event for an unknown handle dropped"
                        ),
                    }
                }
            } else if parsed.as_ref().is_some_and(|v| v.get("op").is_none()) {
                settle_reply(&pending, &handlers, &events, trimmed);
            }
        }
        // The socket closed: drop every pending sender so any in-flight
        // register() waiter returns OrzmaError::Disconnected instead of blocking
        // forever on a reply that will never arrive.
        if let Ok(mut q) = pending.lock() {
            q.clear();
        }
        disconnected.store(true, Ordering::Relaxed);
    });
}

/// Settles the oldest outstanding request with one op-less reply line.
fn settle_reply(
    pending: &PendingReplies,
    handlers: &HandlerRegistry,
    events: &EventRegistry,
    line: &str,
) {
    let Ok(reply) = serde_json::from_str::<ServerReply>(line) else {
        return;
    };
    // NOTE: pop before validating. Leaving the entry on a shape mismatch would
    // desync the queue permanently, since replies arrive strictly in request
    // order.
    let Some(entry) = pending.lock().ok().and_then(|mut q| q.pop_front()) else {
        return;
    };
    match entry {
        Pending::Register {
            reply: waiter,
            handlers: methods,
            events: queues,
        } => {
            let outcome = match (reply.ok, reply.handle, reply.instance) {
                (true, Some(handle), Some(instance)) => {
                    let key = handle.to_string();
                    // NOTE: install on this thread, before the next line is
                    // read, so a `call` or `event` pipelined right behind the
                    // reply finds its handlers and queues rather than racing
                    // the registrant's thread.
                    if let Ok(mut map) = handlers.lock() {
                        map.insert(key.clone(), methods);
                    }
                    if let Ok(mut map) = events.lock() {
                        map.insert(key, queues);
                    }
                    Ok((handle, instance))
                }
                (true, _, _) => Err(OrzmaError::Register {
                    reason: "register reply missing handle or instance".into(),
                }),
                (false, _, _) => Err(OrzmaError::Register {
                    reason: reply.error.unwrap_or_else(|| "unknown".into()),
                }),
            };
            let _ = waiter.send(outcome);
        }
        Pending::NewInstance { reply: waiter } => {
            let outcome = match (reply.ok, reply.instance) {
                (true, Some(instance)) => Ok(instance),
                (true, None) => Err(OrzmaError::Instance {
                    reason: "new_instance reply missing instance".into(),
                }),
                (false, _) => Err(OrzmaError::Instance {
                    reason: reply.error.unwrap_or_else(|| "unknown".into()),
                }),
            };
            let _ = waiter.send(outcome);
        }
    }
}

/// Queues `entry` and writes its request line, both under the writer lock.
///
/// # Invariants
///
/// The pending entry is pushed while the writer lock is held so the queue order
/// matches the on-wire order; a reply carries no request id, so a concurrent
/// requester would otherwise settle this caller's waiter. A failed write pops
/// the entry back off, since no reply will ever arrive for a request that never
/// went out.
fn send_request(
    writer: &SharedWriter,
    pending: &PendingReplies,
    entry: Pending,
    line: &str,
) -> OrzmaResult<()> {
    let mut w = writer.lock()?;
    pending.lock()?.push_back(entry);
    if let Err(e) = writeln!(w, "{line}").and_then(|()| w.flush()) {
        pending.lock()?.pop_back();
        return Err(e.into());
    }
    Ok(())
}

/// Blocks for one request's reply, reporting both a closed channel and an
/// expired [`REPLY_TIMEOUT`] as [`OrzmaError::Disconnected`].
fn await_reply<T>(rx: &Receiver<OrzmaResult<T>>) -> OrzmaResult<T> {
    rx.recv_timeout(REPLY_TIMEOUT)
        .unwrap_or(Err(OrzmaError::Disconnected))
}

fn dispatch_call(writer: &SharedWriter, handlers: &HandlerRegistry, call: IncomingCall) {
    tracing::debug!(
        handle = call.handle,
        instance = call.instance,
        method = call.method,
        "dispatching an inbound call"
    );
    let handler = handlers
        .lock()
        .ok()
        .and_then(|map| map.get(&call.handle).cloned())
        .and_then(|methods| methods.get(&call.method).cloned());

    let result = match handler {
        // A user handler runs on this reader thread; isolate panics so one bad
        // handler can't unwind the thread and silence all future RPC + register
        // replies. A panicked handler reports as a rejected call.
        Some(h) => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h(call.params))) {
                Ok(r) => r.map_err(|e| e.message().to_owned()),
                Err(_) => Err("handler panicked".to_owned()),
            }
        }
        None => Err("unknown_method".to_owned()),
    };

    let msg = ClientMsg::Reply {
        req_id: call.req_id,
        result,
    };
    if let Ok(line) = serde_json::to_string(&msg)
        && let Ok(mut w) = writer.lock()
    {
        let _ = writeln!(w, "{line}");
        let _ = w.flush();
    }
}

fn attempt_reconnect(
    writer: &SharedWriter,
    handlers: &HandlerRegistry,
    pending: &PendingReplies,
    pending_compositing: &PendingCompositing,
    disconnected: &Arc<AtomicBool>,
    generation: &Arc<AtomicU64>,
    registrations: &Arc<Mutex<Vec<Registration>>>,
    events: &EventRegistry,
    token: &str,
) {
    let Some(sock) = resolve_orzma_sock() else {
        tracing::debug!("reconnect: ORZMA_SOCK is unset, will retry on next signal");
        return;
    };
    // NOTE: claim the registrations before dialing, and hold them through the
    // replay. A mint holds the same lock across its own round trip, so taking
    // it any later would let this swap the writer under a request the mint has
    // already prepared for the old connection — which the new one answers with
    // `unknown_handle`.
    let Ok(regs) = registrations.lock() else {
        return;
    };
    let new_stream = match connect_sock(&sock) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("reconnect: socket unavailable: {e}");
            return;
        }
    };
    let cloned = match new_stream.try_clone() {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("reconnect: try_clone failed: {e}");
            return;
        }
    };
    {
        let Ok(mut w) = writer.lock() else { return };
        *w = cloned;
    }
    {
        let line = match serde_json::to_string(&ClientMsg::Hello {
            token: token.to_owned(),
        }) {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!("reconnect: serialize hello failed: {e}");
                return;
            }
        };
        let Ok(mut w) = writer.lock() else { return };
        if writeln!(w, "{line}").and_then(|()| w.flush()).is_err() {
            tracing::debug!("reconnect: hello write failed");
            return;
        }
    }
    disconnected.store(false, Ordering::Relaxed);
    spawn_reader(
        new_stream,
        writer.clone(),
        handlers.clone(),
        pending.clone(),
        pending_compositing.clone(),
        events.clone(),
        disconnected.clone(),
    );
    for reg in regs.iter() {
        if !replay_registration(writer, handlers, pending, events, reg) {
            disconnected.store(true, Ordering::Relaxed);
            return;
        }
    }
    generation.fetch_add(1, Ordering::Relaxed);
    tracing::debug!("reconnect: completed successfully");
}

/// Re-registers one saved registration on the fresh connection and refills
/// every id slot it owns — the handle, the default instance, and one
/// `new_instance` round trip per extra instance — returning whether the whole
/// replay succeeded.
///
/// A partial replay leaves the slots it already refilled in place. The caller
/// marks the session disconnected without bumping the generation, so the next
/// attempt replays this registration from the start.
fn replay_registration(
    writer: &SharedWriter,
    handlers: &HandlerRegistry,
    pending: &PendingReplies,
    events: &EventRegistry,
    reg: &Registration,
) -> bool {
    let (tx, rx) = bounded(1);
    let line = match serde_json::to_string(&ClientMsg::Register(reg.kind.clone())) {
        Ok(l) => l,
        Err(e) => {
            tracing::debug!("reconnect: serialize register failed: {e}");
            return false;
        }
    };
    if let Err(e) = send_request(
        writer,
        pending,
        Pending::Register {
            reply: tx,
            handlers: reg.handlers.clone(),
            events: reg.events.clone(),
        },
        &line,
    ) {
        tracing::debug!("reconnect: register write failed: {e}");
        return false;
    }
    let (new_handle, new_instance) = match await_reply(&rx) {
        Ok(minted) => minted,
        Err(e) => {
            tracing::debug!("reconnect: re-registration failed: {e}");
            return false;
        }
    };
    let old = reg.handle_id();
    let key = new_handle.to_string();
    if let Ok(mut map) = handlers.lock() {
        map.remove(old.as_str());
        map.insert(key.clone(), reg.handlers.clone());
    }
    if let Ok(mut map) = events.lock() {
        map.remove(old.as_str());
        map.insert(key, reg.events.clone());
    }
    *reg.instance_slot.lock().unwrap_or_else(|e| e.into_inner()) = new_instance;
    let handle_id = new_handle;
    *reg.handle_slot.lock().unwrap_or_else(|e| e.into_inner()) = handle_id.clone();

    for slot in &reg.extra_instances {
        let (tx, rx) = bounded(1);
        let line = match serde_json::to_string(&ClientMsg::NewInstance {
            handle: handle_id.clone(),
        }) {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!("reconnect: serialize new_instance failed: {e}");
                return false;
            }
        };
        if let Err(e) = send_request(writer, pending, Pending::NewInstance { reply: tx }, &line) {
            tracing::debug!("reconnect: new_instance write failed: {e}");
            return false;
        }
        match await_reply(&rx) {
            Ok(instance) => *slot.lock().unwrap_or_else(|e| e.into_inner()) = instance,
            Err(e) => {
                tracing::debug!("reconnect: re-minting an instance failed: {e}");
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use std::fmt::Debug;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    const INSTANCE_A: &str = "3f5a9c02d1e84b7690ab3cde12f45678";
    const INSTANCE_B: &str = "81b4e77c05a3492fd6180e29ba735fc1";

    /// Serializes the tests that write `$ORZMA_SOCK`, which is process-global.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    /// A `tracing` subscriber that renders every DEBUG event's fields into a
    /// shared buffer, so a test can assert on a diagnostic the code emits
    /// instead of returns. Install it with `tracing::subscriber::with_default`,
    /// which scopes it to the calling thread.
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<String>>>);

    impl Subscriber for CapturedLogs {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            *metadata.level() == Level::DEBUG
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut rendered = String::new();
            event.record(&mut FieldText(&mut rendered));
            self.0
                .lock()
                .expect("the capture buffer is uncontended")
                .push(rendered);
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    struct FieldText<'a>(&'a mut String);

    impl Visit for FieldText<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
            self.0.push_str(&format!("{}={value:?} ", field.name()));
        }
    }

    /// Asserts that a register waiter and a new-instance waiter each receive
    /// their own reply when both are outstanding, so a mismatch cannot desync
    /// the queue.
    ///
    /// Case: one thread registers a second view while another is already
    /// asking for an extra placement.
    #[test]
    fn a_mixed_pending_queue_routes_each_reply_to_its_waiter() {
        let pending: PendingReplies = Arc::new(Mutex::new(VecDeque::new()));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let events: EventRegistry = Arc::new(Mutex::new(HashMap::new()));
        let (reg_tx, reg_rx) = bounded(1);
        let (inst_tx, inst_rx) = bounded(1);
        pending.lock().unwrap().push_back(Pending::Register {
            reply: reg_tx,
            handlers: Arc::new(HashMap::new()),
            events: Arc::new(EventQueues::from_decls(&[])),
        });
        pending
            .lock()
            .unwrap()
            .push_back(Pending::NewInstance { reply: inst_tx });

        settle_reply(
            &pending,
            &handlers,
            &events,
            r#"{"ok":true,"handle":"h","instance":"i1"}"#,
        );
        settle_reply(
            &pending,
            &handlers,
            &events,
            r#"{"ok":true,"instance":"i2"}"#,
        );

        assert_eq!(
            reg_rx.recv().unwrap().unwrap(),
            (HandleId::from("h".to_string()), "i1".to_string())
        );
        assert_eq!(inst_rx.recv().unwrap().unwrap(), "i2".to_string());
        assert!(pending.lock().unwrap().is_empty());
    }

    /// Asserts that every reply a waiting request cannot be satisfied by — a
    /// rejection, or a success missing the id it should carry — still consumes
    /// that request's queue entry and fails only its own waiter.
    ///
    /// Case: the host rejects a `new_instance` for a handle this connection
    /// does not own, then answers two more requests malformed, while a further
    /// request waits behind them.
    #[test]
    fn an_unusable_reply_consumes_its_entry_rather_than_desyncing_the_queue() {
        let pending: PendingReplies = Arc::new(Mutex::new(VecDeque::new()));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let events: EventRegistry = Arc::new(Mutex::new(HashMap::new()));
        let (rejected_tx, rejected_rx) = bounded(1);
        let (no_instance_tx, no_instance_rx) = bounded(1);
        let (no_handle_tx, no_handle_rx) = bounded(1);
        let (survivor_tx, survivor_rx) = bounded(1);
        {
            let mut q = pending.lock().unwrap();
            q.push_back(Pending::NewInstance { reply: rejected_tx });
            q.push_back(Pending::NewInstance {
                reply: no_instance_tx,
            });
            q.push_back(Pending::Register {
                reply: no_handle_tx,
                handlers: Arc::new(HashMap::new()),
                events: Arc::new(EventQueues::from_decls(&[])),
            });
            q.push_back(Pending::NewInstance { reply: survivor_tx });
        }

        settle_reply(
            &pending,
            &handlers,
            &events,
            r#"{"ok":false,"error":"not_owner"}"#,
        );
        settle_reply(&pending, &handlers, &events, r#"{"ok":true}"#);
        settle_reply(
            &pending,
            &handlers,
            &events,
            r#"{"ok":true,"instance":"i3"}"#,
        );
        settle_reply(
            &pending,
            &handlers,
            &events,
            r#"{"ok":true,"instance":"i4"}"#,
        );

        assert!(matches!(
            rejected_rx.recv().unwrap(),
            Err(OrzmaError::Instance { .. })
        ));
        assert!(matches!(
            no_instance_rx.recv().unwrap(),
            Err(OrzmaError::Instance { .. })
        ));
        assert!(matches!(
            no_handle_rx.recv().unwrap(),
            Err(OrzmaError::Register { .. })
        ));
        assert_eq!(survivor_rx.recv().unwrap().unwrap(), "i4".to_string());
        assert!(pending.lock().unwrap().is_empty());
    }

    /// A session holding one saved registration, returned with a handle over
    /// that registration's id slots and the far end of its socket. No reader
    /// thread runs, so a request the session writes waits until the test
    /// settles it by hand.
    fn session_with_one_registration() -> (Orzma, WebviewHandle, UnixStream) {
        let (client, server) = UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client));
        let handle_slot = Arc::new(Mutex::new(HandleId::from("h-old".to_owned())));
        let instance_slot = Arc::new(Mutex::new(INSTANCE_A.to_owned()));
        let events = Arc::new(EventQueues::from_decls(&[]));
        let core = Arc::new(SessionCore {
            pending: Arc::new(Mutex::new(VecDeque::new())),
            registrations: Arc::new(Mutex::new(vec![Registration {
                kind: Webview::inline("x").kind,
                handle_slot: handle_slot.clone(),
                instance_slot: instance_slot.clone(),
                extra_instances: Vec::new(),
                handlers: Arc::new(HashMap::new()),
                events: events.clone(),
            }])),
        });
        let handle = WebviewHandle::new_shared(
            handle_slot,
            instance_slot,
            events,
            writer.clone(),
            Arc::downgrade(&core),
        );
        let orzma = Orzma {
            writer,
            core,
            frame: Arc::new(Mutex::new(FramePlacements::default())),
            pending_compositing: Arc::new(Mutex::new(HashMap::new())),
            disconnected: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            reconnect_tx: bounded::<()>(1).0,
        };
        (orzma, handle, server)
    }

    /// Asserts that an instance minted while a reconnect replaces the handle it
    /// was requested under is still recorded for replay.
    ///
    /// Case: orzma restarts in the moment between the host answering an app's
    /// request for a second placement and the app taking delivery of it.
    #[test]
    fn a_mint_racing_a_reconnect_still_records_the_placement_for_replay() {
        let (orzma, handle, server) = session_with_one_registration();
        let refilled = orzma.core.registrations.lock().unwrap()[0]
            .handle_slot
            .clone();
        let pending = orzma.core.pending.clone();

        let host = thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(server).read_line(&mut line).unwrap();
            *refilled.lock().unwrap() = HandleId::from("h-new".to_owned());
            settle_reply(
                &pending,
                &Arc::new(Mutex::new(HashMap::new())),
                &Arc::new(Mutex::new(HashMap::new())),
                &format!(r#"{{"ok":true,"instance":"{INSTANCE_B}"}}"#),
            );
        });

        let extra = handle.new_instance().unwrap();
        host.join().unwrap();

        assert_eq!(extra.id(), INSTANCE_B);
        assert_eq!(
            orzma.core.registrations.lock().unwrap()[0]
                .extra_instances
                .len(),
            1,
            "the minted placement must be recorded for replay"
        );
    }

    /// Asserts that a mint holds the saved registrations from before it sends
    /// its request until after it records the placement, so a reconnect replay
    /// can neither run between the two nor answer the request itself.
    ///
    /// Case: the socket dies while an app is asking for a second placement, and
    /// the reconnect thread reaches the registrations it has to replay.
    #[test]
    fn a_reconnect_replay_cannot_interleave_with_a_mint_in_flight() {
        let (orzma, handle, server) = session_with_one_registration();
        let pending = orzma.core.pending.clone();
        let registrations = orzma.core.registrations.clone();

        let host = thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(server).read_line(&mut line).unwrap();
            let held_while_in_flight = registrations.try_lock().is_err();
            settle_reply(
                &pending,
                &Arc::new(Mutex::new(HashMap::new())),
                &Arc::new(Mutex::new(HashMap::new())),
                &format!(r#"{{"ok":true,"instance":"{INSTANCE_B}"}}"#),
            );
            held_while_in_flight
        });

        handle.new_instance().unwrap();

        assert!(
            host.join().unwrap(),
            "the request went out with the registrations unlocked, leaving a replay free to interleave"
        );
    }

    /// Asserts that a handle mints an extra placement of its own registration
    /// and records it for replay, without the caller holding the session.
    ///
    /// Case: an app splits a pane and shows the view it is already drawing a
    /// second time alongside the first.
    #[test]
    fn a_handle_mints_its_own_placement() {
        let (orzma, handle, server) = session_with_one_registration();
        let pending = orzma.core.pending.clone();

        let host = thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(server).read_line(&mut line).unwrap();
            settle_reply(
                &pending,
                &Arc::new(Mutex::new(HashMap::new())),
                &Arc::new(Mutex::new(HashMap::new())),
                &format!(r#"{{"ok":true,"instance":"{INSTANCE_B}"}}"#),
            );
        });

        let extra = handle.new_instance().unwrap();
        host.join().unwrap();

        assert_eq!(extra.id(), INSTANCE_B);
        assert_eq!(
            orzma.core.registrations.lock().unwrap()[0]
                .extra_instances
                .len(),
            1,
            "the minted placement must be recorded for replay"
        );
    }

    /// Asserts that a handle whose session is gone reports the closed session
    /// rather than writing a request nothing is left to track.
    ///
    /// Case: an app tears its session down while a widget still holds the
    /// handle it was drawing.
    #[test]
    fn a_handle_outliving_its_session_cannot_mint() {
        let (orzma, handle, _server) = session_with_one_registration();
        drop(orzma);

        assert!(matches!(
            handle.new_instance(),
            Err(OrzmaError::SessionClosed)
        ));
    }

    /// Asserts that a reconnect claims the saved registrations before it dials
    /// the new socket, so a mint already holding them cannot have the writer
    /// swapped out from under the request it is about to send.
    ///
    /// Case: orzma restarts in the moment an app is asking for a second
    /// placement of a view it registered earlier.
    #[test]
    fn a_reconnect_waits_for_the_registrations_before_it_dials() {
        use crate::uds::UnixListener;
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("new.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        listener.set_nonblocking(true).unwrap();
        // SAFETY: ENV_LOCK is held, serializing every test in this module that
        // writes the environment.
        unsafe { std::env::set_var("ORZMA_SOCK", &sock_path) };

        let (old_client, _old_server) = UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(old_client));
        let pending: PendingReplies = Arc::new(Mutex::new(VecDeque::new()));
        let pending_compositing: PendingCompositing = Arc::new(Mutex::new(HashMap::new()));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let events: EventRegistry = Arc::new(Mutex::new(HashMap::new()));
        let registrations: Arc<Mutex<Vec<Registration>>> = Arc::new(Mutex::new(Vec::new()));
        let disconnected = Arc::new(AtomicBool::new(true));
        let generation = Arc::new(AtomicU64::new(0));

        let held = registrations.lock().unwrap();
        let reconnect = {
            let (writer, pending, pending_compositing, handlers, events, registrations) = (
                writer.clone(),
                pending.clone(),
                pending_compositing.clone(),
                handlers.clone(),
                events.clone(),
                registrations.clone(),
            );
            let (disconnected, generation) = (disconnected.clone(), generation.clone());
            thread::spawn(move || {
                attempt_reconnect(
                    &writer,
                    &handlers,
                    &pending,
                    &pending_compositing,
                    &disconnected,
                    &generation,
                    &registrations,
                    &events,
                    "tok",
                );
            })
        };

        thread::sleep(Duration::from_millis(250));
        assert!(
            matches!(listener.accept(), Err(e) if e.kind() == ErrorKind::WouldBlock),
            "the reconnect dialed the new socket while the registrations were held"
        );

        drop(held);
        listener.set_nonblocking(false).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        assert!(
            line.contains(r#""op":"hello""#),
            "expected the handshake on the new socket, got {line:?}"
        );
        reconnect.join().unwrap();
    }

    /// Asserts that two instances of one registration recorded in the same
    /// frame each get their own mount, rather than the later one replacing
    /// the earlier.
    ///
    /// Case: an app shows the same view in a split, side by side.
    #[test]
    fn two_instances_of_one_registration_each_mount() {
        let a = INSTANCE_A;
        let b = INSTANCE_B;
        let mut state = FlushState::default();
        let mut frame = FramePlacements::default();
        frame.record(a.into(), rect(0, 0, 10, 5));
        frame.record(b.into(), rect(0, 6, 10, 5));

        let mut out = Vec::new();
        flush_placements(&mut out, &mut state, frame.placements_for_test()).unwrap();
        let written = String::from_utf8(out).unwrap();
        assert!(written.contains(&format!("n={a}")));
        assert!(written.contains(&format!("n={b}")));
        assert_eq!(state.last.len(), 2);
    }

    /// Asserts that only the instance whose rect moved is re-mounted.
    ///
    /// Case: a split is dragged, resizing one pane and leaving the other.
    #[test]
    fn only_the_moved_instance_is_remounted() {
        let a = INSTANCE_A;
        let b = INSTANCE_B;
        let mut state = FlushState::default();
        let first = vec![
            Placement {
                instance: a.into(),
                area: rect(0, 0, 10, 5),
            },
            Placement {
                instance: b.into(),
                area: rect(0, 6, 10, 5),
            },
        ];
        flush_placements(&mut Vec::new(), &mut state, &first).unwrap();

        let second = vec![
            Placement {
                instance: a.into(),
                area: rect(0, 0, 10, 5),
            },
            Placement {
                instance: b.into(),
                area: rect(0, 6, 10, 8),
            },
        ];
        let mut out = Vec::new();
        flush_placements(&mut out, &mut state, &second).unwrap();
        let written = String::from_utf8(out).unwrap();
        assert!(
            !written.contains(&format!("n={a}")),
            "the unchanged instance is not re-mounted"
        );
        assert!(written.contains(&format!("n={b}")));
    }

    #[test]
    fn flush_state_reset_clears_placements_and_focus() {
        let mut state = FlushState::default();
        state.last.insert("h1".into(), rect(0, 0, 10, 5));
        state.last_focused = Some("h1".into());
        state.reset();
        assert!(state.last.is_empty(), "last should be empty after reset");
        assert_eq!(
            state.last_focused, None,
            "last_focused should be None after reset"
        );
    }

    /// Asserts that a placement is mounted at its cursor position when new or
    /// moved, and emits nothing on a frame where it did not move.
    ///
    /// Case: an app draws the same webview pane for several frames, then the
    /// user widens the window.
    #[test]
    fn flush_emits_mount_then_skips_unchanged() {
        let mut placements = vec![Placement {
            instance: INSTANCE_A.into(),
            area: rect(2, 3, 48, 12),
        }];
        let mut state = FlushState::default();

        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &placements).unwrap();
        let first = String::from_utf8(buf).unwrap();
        assert!(first.contains("\x1b[4;3H"));
        assert!(first.contains(&format!("Omount;n={INSTANCE_A},r=12,c=48")));

        let mut buf2 = Vec::new();
        flush_placements(&mut buf2, &mut state, &placements).unwrap();
        assert!(
            String::from_utf8(buf2).unwrap().is_empty(),
            "unchanged frame emits nothing"
        );

        placements[0].area = rect(2, 3, 50, 12);
        let mut buf3 = Vec::new();
        flush_placements(&mut buf3, &mut state, &placements).unwrap();
        assert!(
            String::from_utf8(buf3)
                .unwrap()
                .contains(&format!("Omount;n={INSTANCE_A},r=12,c=50"))
        );
    }

    /// Asserts that an instance drawn last frame but not this one is unmounted.
    ///
    /// Case: the user closes the pane that held the only placement of a view.
    #[test]
    fn flush_unmounts_vanished_instance() {
        let mut state = FlushState::default();
        let placements = vec![Placement {
            instance: INSTANCE_A.into(),
            area: rect(0, 0, 10, 5),
        }];
        flush_placements(&mut Vec::new(), &mut state, &placements).unwrap();

        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &[]).unwrap();
        assert!(
            String::from_utf8(buf)
                .unwrap()
                .contains(&format!("Ounmount;n={INSTANCE_A}"))
        );
    }

    /// Asserts that the socket flush writes one `mount` op line per new
    /// placement, carrying the 0-based cell and the clamped size, and
    /// writes nothing for an unchanged frame.
    ///
    /// Case: orzmd in a Windows pane draws its webview for the first time,
    /// then redraws with nothing moved.
    #[test]
    fn socket_flush_emits_a_mount_op_then_skips_unchanged() {
        let placements = vec![Placement {
            instance: INSTANCE_A.into(),
            area: rect(2, 3, 48, 12),
        }];
        let mut state = FlushState::default();

        let mut buf = Vec::new();
        flush_placements_over_socket(&mut buf, &mut state, &placements).unwrap();
        let line: serde_json::Value =
            serde_json::from_str(String::from_utf8(buf).unwrap().trim()).unwrap();
        assert_eq!(
            line,
            serde_json::json!({
                "op": "mount",
                "instance": INSTANCE_A,
                "row": 3,
                "col": 2,
                "rows": 12,
                "cols": 48
            })
        );

        let mut buf2 = Vec::new();
        flush_placements_over_socket(&mut buf2, &mut state, &placements).unwrap();
        assert!(buf2.is_empty(), "unchanged frame emits nothing");
    }

    /// Asserts that the socket flush writes an `unmount` op for an
    /// instance drawn last frame but absent now.
    ///
    /// Case: the user closes the pane that held the only placement of a
    /// view, in a Windows pane.
    #[test]
    fn socket_flush_unmounts_a_vanished_instance() {
        let mut state = FlushState::default();
        let placements = vec![Placement {
            instance: INSTANCE_A.into(),
            area: rect(0, 0, 10, 5),
        }];
        flush_placements_over_socket(&mut Vec::new(), &mut state, &placements).unwrap();

        let mut buf = Vec::new();
        flush_placements_over_socket(&mut buf, &mut state, &[]).unwrap();
        let line: serde_json::Value =
            serde_json::from_str(String::from_utf8(buf).unwrap().trim()).unwrap();
        assert_eq!(
            line,
            serde_json::json!({ "op": "unmount", "instance": INSTANCE_A })
        );
        assert!(state.last.is_empty());
    }

    /// Asserts that on Windows a connected frame flush sends its geometry
    /// over the control socket and writes nothing to the PTY.
    ///
    /// Case: orzmd draws inside a Windows pane, where ConPTY would drop an
    /// APC written to the PTY.
    #[cfg(windows)]
    #[test]
    fn emit_frame_sends_geometry_over_the_socket_on_windows() {
        let (client, server) = UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client));
        let mut state = FlushState::default();
        let mut frame = FramePlacements::default();
        frame.record(INSTANCE_A.into(), rect(0, 0, 10, 5));

        let mut pty = Vec::new();
        state.emit_frame(&mut pty, &writer, &frame).unwrap();

        assert!(pty.is_empty(), "nothing rides the PTY on Windows");
        let mut line = String::new();
        BufReader::new(server).read_line(&mut line).unwrap();
        assert!(line.contains(r#""op":"mount""#), "got: {line}");
    }

    /// Asserts that on Unix a connected frame flush writes its geometry to
    /// the PTY as APC verbs and sends nothing over the control socket.
    ///
    /// Case: orzmd draws inside a macOS pane.
    #[cfg(not(windows))]
    #[test]
    fn emit_frame_sends_geometry_over_the_pty_on_unix() {
        use std::io::Read;
        let (client, server) = UnixStream::pair().unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client));
        let mut state = FlushState::default();
        let mut frame = FramePlacements::default();
        frame.record(INSTANCE_A.into(), rect(0, 0, 10, 5));

        let mut pty = Vec::new();
        state.emit_frame(&mut pty, &writer, &frame).unwrap();

        assert!(String::from_utf8(pty).unwrap().contains("Omount;n="));
        server.set_nonblocking(true).unwrap();
        let mut probe = [0u8; 1];
        let read = (&server).read(&mut probe);
        assert!(
            matches!(read, Err(ref e) if e.kind() == ErrorKind::WouldBlock),
            "nothing rides the socket on Unix, got {read:?}"
        );
    }

    /// Asserts that a zero-width area is skipped even when the instance is a
    /// well-formed one, so the area gate alone decides.
    ///
    /// Case: a layout collapses a pane to nothing while its widget still
    /// renders.
    #[test]
    fn flush_skips_degenerate_area() {
        let mut state = FlushState::default();
        let placements = vec![Placement {
            instance: INSTANCE_A.into(),
            area: rect(0, 0, 0, 5),
        }];
        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &placements).unwrap();
        assert!(String::from_utf8(buf).unwrap().is_empty());
    }

    /// Asserts that an id which is not a minted instance is skipped rather
    /// than mounted or propagated as an error.
    ///
    /// Case: a caller passes a registration handle where the widget wants an
    /// instance id.
    #[test]
    fn flush_skips_an_id_that_is_not_an_instance() {
        let mut state = FlushState::default();
        let placements = vec![Placement {
            instance: "nf2k7q5w3x3m5a6b2c4d6e7f".into(),
            area: rect(0, 0, 10, 5),
        }];
        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &placements).unwrap();
        assert!(String::from_utf8(buf).unwrap().is_empty());
        assert!(state.last.is_empty());
    }

    /// Asserts that skipping a placement emits one debug event naming the
    /// rejected id, so the skip leaves a trace the caller can find.
    ///
    /// Case: a caller passes a registration handle where the widget wants an
    /// instance id, sees no webview, and turns on debug logging to find out
    /// which id the flush refused.
    #[test]
    fn flush_logs_the_placement_it_skips() {
        let logs = CapturedLogs::default();
        let mut state = FlushState::default();
        let placements = vec![Placement {
            instance: "nf2k7q5w3x3m5a6b2c4d6e7f".into(),
            area: rect(0, 0, 10, 5),
        }];
        let mut buf = Vec::new();

        tracing::subscriber::with_default(logs.clone(), || {
            flush_placements(&mut buf, &mut state, &placements).unwrap();
        });

        let captured = logs.0.lock().expect("the capture buffer is uncontended");
        assert_eq!(captured.len(), 1);
        assert!(captured[0].contains("nf2k7q5w3x3m5a6b2c4d6e7f"));
        assert!(captured[0].contains("skipping a placement"));
    }

    /// Asserts that a focus op names the newly-focused instance, and that a
    /// frame focusing the same instance again writes nothing.
    ///
    /// Case: the user clicks into a webview pane and then keeps typing in it
    /// across many frames.
    #[test]
    fn flush_focus_emits_on_change_and_skips_unchanged() {
        let mut last = None;
        let mut buf = Vec::new();
        flush_focus(&mut buf, &mut last, &Some(INSTANCE_A.to_string())).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(String::from_utf8(buf).unwrap().trim()).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["instance"], INSTANCE_A);

        let mut buf2 = Vec::new();
        flush_focus(&mut buf2, &mut last, &Some(INSTANCE_A.to_string())).unwrap();
        assert!(
            String::from_utf8(buf2).unwrap().is_empty(),
            "unchanged focus emits nothing"
        );
    }

    /// Asserts that dropping focus emits a focus op with a null instance and
    /// clears the remembered target.
    ///
    /// Case: the user moves focus from a webview pane back to a native widget.
    #[test]
    fn flush_focus_emits_blur_on_none() {
        let mut last = Some(INSTANCE_A.to_string());
        let mut buf = Vec::new();
        flush_focus(&mut buf, &mut last, &None).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(String::from_utf8(buf).unwrap().trim()).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["instance"], serde_json::Value::Null);
        assert_eq!(last, None);
    }

    #[test]
    fn take_compositing_returns_and_removes_entry() {
        let mut fp = FramePlacements::default();
        fp.pending_compositing.insert("h1".into(), true);
        assert_eq!(fp.take_compositing("h1"), Some(true));
        assert!(fp.pending_compositing.is_empty());
    }

    #[test]
    fn take_compositing_returns_none_when_absent() {
        let mut fp = FramePlacements::default();
        assert_eq!(fp.take_compositing("missing"), None);
    }

    #[test]
    fn frame_drains_pending_compositing_each_call() {
        let shared: Arc<Mutex<HashMap<String, bool>>> = Arc::new(Mutex::new(HashMap::new()));
        shared.lock().unwrap().insert("h1".into(), true);

        let frame_arc: Arc<Mutex<FramePlacements>> =
            Arc::new(Mutex::new(FramePlacements::default()));

        {
            let mut fp = frame_arc.lock().unwrap_or_else(|e| e.into_inner());
            fp.placements.clear();
            fp.focused = None;
            fp.pending_compositing = shared
                .lock()
                .map(|mut map| std::mem::take(&mut *map))
                .unwrap_or_default();
        }
        {
            let fp = frame_arc.lock().unwrap();
            assert_eq!(fp.pending_compositing_for_test().len(), 1);
        }
        // Second drain: shared is now empty, so pending_compositing is cleared.
        {
            let mut fp = frame_arc.lock().unwrap_or_else(|e| e.into_inner());
            fp.placements.clear();
            fp.focused = None;
            fp.pending_compositing = shared
                .lock()
                .map(|mut map| std::mem::take(&mut *map))
                .unwrap_or_default();
        }
        {
            let fp = frame_arc.lock().unwrap();
            assert!(fp.pending_compositing_for_test().is_empty());
        }
    }

    /// Asserts that a compositing push is buffered under the instance it
    /// names, not the handle it also carries.
    ///
    /// Case: one of two placements of a registration starts painting.
    #[test]
    fn reader_thread_inserts_compositing_into_shared_map() {
        use crate::uds::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let pending_compositing: Arc<Mutex<HashMap<String, bool>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingReplies = Arc::new(Mutex::new(VecDeque::new()));

        let client = UnixStream::connect(&sock_path).unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client.try_clone().unwrap()));
        let (server_conn, _) = listener.accept().unwrap();

        spawn_reader(
            client,
            writer.clone(),
            handlers,
            pending,
            pending_compositing.clone(),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(AtomicBool::new(false)),
        );

        use std::io::Write;
        let mut server = server_conn;
        writeln!(
            server,
            r#"{{"op":"compositing","handle":"h1","instance":"{INSTANCE_A}","active":true}}"#
        )
        .unwrap();
        server.flush().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        let map = pending_compositing.lock().unwrap();
        assert_eq!(map.get(INSTANCE_A), Some(&true));
    }

    /// Asserts that a later push for the same instance replaces the buffered
    /// state rather than being ignored.
    ///
    /// Case: a placement is unmounted after having composited.
    #[test]
    fn reader_thread_updates_compositing_to_false() {
        use crate::uds::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test2.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let pending_compositing: Arc<Mutex<HashMap<String, bool>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingReplies = Arc::new(Mutex::new(VecDeque::new()));

        let client = UnixStream::connect(&sock_path).unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client.try_clone().unwrap()));
        let (server_conn, _) = listener.accept().unwrap();

        spawn_reader(
            client,
            writer.clone(),
            handlers,
            pending,
            pending_compositing.clone(),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(AtomicBool::new(false)),
        );

        use std::io::Write;
        let mut server = server_conn;
        writeln!(
            server,
            r#"{{"op":"compositing","handle":"h1","instance":"{INSTANCE_A}","active":false}}"#
        )
        .unwrap();
        server.flush().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        let map = pending_compositing.lock().unwrap();
        assert_eq!(map.get(INSTANCE_A), Some(&false));
    }

    #[test]
    fn reader_thread_routes_event_into_registered_queues() {
        use crate::events::{EventDecl, EventQueues, EventRegistry};
        use crate::uds::UnixListener;
        use std::any::TypeId;
        use std::io::Write;

        struct Hello;
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("ev.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let decls = vec![EventDecl {
            name: "hello".into(),
            type_id: TypeId::of::<Hello>(),
        }];
        let queues = Arc::new(EventQueues::from_decls(&decls));
        let events: EventRegistry = Arc::new(Mutex::new(HashMap::new()));
        events
            .lock()
            .unwrap()
            .insert("h1".to_owned(), queues.clone());

        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingReplies = Arc::new(Mutex::new(VecDeque::new()));
        let pending_compositing: PendingCompositing = Arc::new(Mutex::new(HashMap::new()));

        let client = UnixStream::connect(&sock_path).unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client.try_clone().unwrap()));
        let (server_conn, _) = listener.accept().unwrap();

        spawn_reader(
            client,
            writer.clone(),
            handlers,
            pending,
            pending_compositing,
            events.clone(),
            Arc::new(AtomicBool::new(false)),
        );

        let mut server = server_conn;
        writeln!(
            server,
            r#"{{"op":"event","handle":"h1","event":"hello","payload":{{"n":7}}}}"#
        )
        .unwrap();
        server.flush().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        assert_eq!(
            queues.drain_type(TypeId::of::<Hello>()),
            vec![serde_json::json!({"n":7})]
        );
    }
}
