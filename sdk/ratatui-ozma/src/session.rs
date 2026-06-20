//! The Ozma session: socket connection, reader thread, flush.

use crate::error::{OzmaError, OzmaResult};
use crate::handler::BoxedHandler;
use crate::osc::{clamp_dims, cursor_to, mount, unmount, valid_handle};
use crate::protocol::{ClientMsg, IncomingCall, RegisterKind, RegisterReply};
use crate::webview::{SharedWriter, Webview, WebviewHandle};
use crossbeam_channel::{Sender, bounded};
use ratatui::layout::Rect;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

/// One webview's requested position this frame.
#[derive(Debug, Clone)]
pub(crate) struct Placement {
    pub(crate) handle: String,
    pub(crate) area: Rect,
}

/// The per-frame collector handed to the [`crate::WebviewWidget`] as its state.
#[derive(Debug, Default)]
pub struct FramePlacements {
    placements: Vec<Placement>,
    focused: Option<String>,
    pub(crate) pending_compositing: HashMap<String, bool>,
}

impl FramePlacements {
    pub(crate) fn record(&mut self, handle: String, area: Rect) {
        self.placements.push(Placement { handle, area });
    }

    /// Marks `handle` focused for this frame. Last writer wins; a debug build
    /// trips an assertion if more than one widget claims focus in a single frame
    /// (the app must focus at most one webview at a time).
    pub(crate) fn set_focused(&mut self, handle: String) {
        debug_assert!(
            self.focused.is_none(),
            "multiple webviews marked focused in one frame (last wins): had {:?}, now {handle:?}",
            self.focused
        );
        self.focused = Some(handle);
    }

    /// Removes and returns the buffered compositing state for `handle`, if any.
    pub(crate) fn take_compositing(&mut self, handle: &str) -> Option<bool> {
        self.pending_compositing.remove(handle)
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

/// Last-emitted geometry per handle, for diff-driven flush.
#[derive(Debug, Default)]
pub(crate) struct FlushState {
    #[cfg(test)]
    pub(crate) last: HashMap<String, Rect>,
    #[cfg(not(test))]
    last: HashMap<String, Rect>,
    last_focused: Option<String>,
}

impl FlushState {
    /// Emits this frame's geometry (mount/unmount OSC) to `out` and, when focus
    /// changed since the last frame, the control-plane focus op to `socket`.
    pub(crate) fn emit_frame(
        &mut self,
        out: &mut impl Write,
        socket: &SharedWriter,
        frame: &FramePlacements,
    ) -> OzmaResult<()> {
        flush_placements(out, self, &frame.placements)?;
        // NOTE: only take the writer lock (shared with the reader thread and
        // every WebviewHandle::emit) when focus actually changed; this runs every
        // render frame and the unchanged path must not contend the lock.
        if self.last_focused == frame.focused {
            return Ok(());
        }
        let mut w = socket.lock()?;
        flush_focus(&mut *w, &mut self.last_focused, &frame.focused)
    }

    pub(crate) fn reset(&mut self) {
        self.last.clear();
        self.last_focused = None;
    }
}

type HandlerRegistry = Arc<Mutex<HashMap<String, Arc<HashMap<String, BoxedHandler>>>>>;
type PendingRegisters = Arc<Mutex<VecDeque<PendingRegister>>>;

/// One in-flight `register` awaiting its untagged reply: the oneshot to wake the
/// caller, plus the handlers to install once the control plane mints the handle.
struct PendingRegister {
    reply: Sender<OzmaResult<String>>,
    handlers: Arc<HashMap<String, BoxedHandler>>,
}

type PendingCompositing = Arc<Mutex<HashMap<String, bool>>>;

/// A saved webview registration for replay on reconnect.
struct Registration {
    kind: RegisterKind,
    handle_slot: Arc<Mutex<String>>,
    handlers: Arc<HashMap<String, BoxedHandler>>,
}

/// The minimal shared state passed to [`OzmaBackend`] for reconnect signalling.
pub(crate) struct ReconnectHandle {
    pub(crate) disconnected: Arc<AtomicBool>,
    pub(crate) generation: Arc<AtomicU64>,
    pub(crate) reconnect_tx: Sender<()>,
}

/// An ozmux session: owns the control-socket connection and reader thread.
pub struct Ozma {
    writer: SharedWriter,
    pending: PendingRegisters,
    frame: Arc<Mutex<FramePlacements>>,
    pending_compositing: PendingCompositing,
    disconnected: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    registrations: Arc<Mutex<Vec<Registration>>>,
    reconnect_tx: crossbeam_channel::Sender<()>,
}

impl Ozma {
    /// Connects to the ozma control socket, performs the `hello` handshake, and
    /// spawns the background reader thread.
    ///
    /// The socket path is resolved from an ordered candidate list (see
    /// [`resolve_ozma_sock_candidates`]): the tmux session environment first — ozmux
    /// rewrites it on every attach, so it names the currently-attached ozmux — then
    /// the inherited `$OZMA_SOCK`, which can be a stale snapshot from an ozmux that
    /// has since exited (the pane forked while that value was live). `connect()`
    /// tries each in order via [`connect_first_reachable`] and uses the first that is
    /// actually reachable, so a stale candidate cannot shadow the live one.
    pub fn connect() -> OzmaResult<Self> {
        let candidates = resolve_ozma_sock_candidates();
        if candidates.is_empty() {
            return Err(OzmaError::NotInPane("OZMA_SOCK"));
        }
        let token = pane_identity(
            std::env::var("OZMA_TOKEN").ok(),
            std::env::var("TMUX_PANE").ok(),
        )
        .ok_or(OzmaError::NotInPane("OZMA_TOKEN or TMUX_PANE"))?;
        let stream = connect_first_reachable(&candidates)?;
        let writer: SharedWriter = Arc::new(Mutex::new(stream.try_clone()?));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingRegisters = Arc::new(Mutex::new(VecDeque::new()));
        let pending_compositing: PendingCompositing = Arc::new(Mutex::new(HashMap::new()));
        let disconnected: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let generation: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
        let registrations: Arc<Mutex<Vec<Registration>>> = Arc::new(Mutex::new(Vec::new()));
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
                        &token2,
                    );
                }
            });
        }

        Ok(Self {
            writer,
            pending,
            frame: Arc::new(Mutex::new(FramePlacements::default())),
            pending_compositing,
            disconnected,
            generation,
            registrations,
            reconnect_tx,
        })
    }

    /// Registers a webview, blocking until the control plane mints its handle.
    pub fn register(&self, webview: Webview) -> OzmaResult<WebviewHandle> {
        let Webview { kind, handlers } = webview;
        let handlers = Arc::new(handlers);
        let (tx, rx) = bounded(1);
        let line = serde_json::to_string(&ClientMsg::Register(kind.clone()))?;
        {
            let mut w = self.writer.lock()?;
            // NOTE: push the pending entry while holding the writer lock so the
            // FIFO order matches the on-wire order — register replies are untagged,
            // so concurrent registrants would otherwise mismatch their handles.
            self.pending.lock()?.push_back(PendingRegister {
                reply: tx,
                handlers: handlers.clone(),
            });
            if let Err(e) = writeln!(w, "{line}").and_then(|()| w.flush()) {
                // The register never went out, so no reply will arrive for this
                // entry; drop it so it can't consume a later registrant's reply.
                self.pending.lock()?.pop_back();
                return Err(e.into());
            }
        }

        let handle = rx.recv().map_err(|_| OzmaError::Disconnected)??;
        let handle_slot = Arc::new(Mutex::new(handle));
        if let Ok(mut regs) = self.registrations.lock() {
            regs.push(Registration {
                kind,
                handle_slot: handle_slot.clone(),
                handlers: handlers.clone(),
            });
        }
        Ok(WebviewHandle::new_shared(handle_slot, self.writer.clone()))
    }

    /// Locks and clears the per-frame placement collector for `render_stateful_widget`.
    ///
    /// The returned guard derefs to [`FramePlacements`]; pass `&mut *ozma.frame()`
    /// as the widget state. Let it drop at the end of the `terminal.draw` closure
    /// so the [`crate::OzmaBackend`] can read the frame during that draw's flush.
    /// Drains any pending compositing notifications from the reader thread into
    /// the frame so widget code can read them via [`FramePlacements::take_compositing`].
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

    /// Returns a handle to the reconnect machinery for use by [`crate::OzmaBackend`].
    pub(crate) fn reconnect_handle(&self) -> ReconnectHandle {
        ReconnectHandle {
            disconnected: self.disconnected.clone(),
            generation: self.generation.clone(),
            reconnect_tx: self.reconnect_tx.clone(),
        }
    }
}

/// Resolves the identity sent in the `hello` handshake: the legacy per-surface
/// `$OZMA_TOKEN` (direct-PTY backend) when set, else the tmux pane id
/// `$TMUX_PANE`. tmux injects `$TMUX_PANE` into every pane it spawns, so the
/// fallback covers the tmux backend where `$OZMA_TOKEN` is never set. `None`
/// when neither is present — the process is not inside an ozmux pane.
fn pane_identity(ozmux_token: Option<String>, tmux_pane: Option<String>) -> Option<String> {
    ozmux_token.filter(|t| !t.is_empty()).or(tmux_pane)
}

/// Extracts the tmux server socket path from a `$TMUX` value
/// (`<socket-path>,<server-pid>,<session-id>`): everything up to the first
/// comma. `None` for an empty value or one starting with a comma, mirroring
/// tmux's own `$TMUX` validity guard so a malformed value cannot resolve to an
/// empty socket path.
fn socket_from_tmux(tmux: &str) -> Option<&str> {
    tmux.split(',').next().filter(|first| !first.is_empty())
}

/// Reads the value of `key` from `tmux show-environment` output. tmux prints one
/// `KEY=value` line per variable and `-KEY` for an unset one; returns the value
/// when a `KEY=` line is present, else `None`.
fn parse_show_environment<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    output.lines().find_map(|line| line.strip_prefix(&prefix))
}

/// The control-socket candidates to try, most-authoritative first.
///
/// The tmux session value (see [`resolve_from_tmux`]) comes first: ozmux rewrites
/// it on every attach via `set-environment`, so it names the currently-attached
/// ozmux. The inherited `$OZMA_SOCK` process env is second — it is the only source
/// under the direct-PTY backend (no `$TMUX`), but in a tmux pane it can be a STALE
/// snapshot from an ozmux that has since exited (the pane forked while that value
/// was live), so it must not shadow the fresh tmux value. Deduped to avoid a
/// redundant connect when the pane forked during the current ozmux (both equal).
fn resolve_ozma_sock_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(sock) = resolve_from_tmux() {
        candidates.push(sock);
    }
    if let Some(sock) = std::env::var("OZMA_SOCK").ok().filter(|s| !s.is_empty())
        && !candidates.contains(&sock)
    {
        candidates.push(sock);
    }
    candidates
}

/// Recovers `$OZMA_SOCK` from the tmux session environment via
/// `tmux -S <socket> show-environment OZMA_SOCK`.
///
/// A pane that forked before ozma ran `set-environment` never inherited
/// `$OZMA_SOCK`, and a running process's environment cannot be changed from
/// outside; tmux injects `$TMUX` into every pane, so reading the value back
/// recovers it without a shell-rc hook. `None` outside tmux, when tmux is
/// unavailable, or when the variable is unset on the session.
fn resolve_from_tmux() -> Option<String> {
    let tmux = std::env::var("TMUX").ok()?;
    let socket = socket_from_tmux(&tmux)?;
    let output = std::process::Command::new("tmux")
        .args(["-S", socket, "show-environment", "OZMA_SOCK"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_show_environment(&stdout, "OZMA_SOCK")
        .filter(|sock| !sock.is_empty())
        .map(str::to_owned)
}

/// Connects to the first reachable candidate, treating an unreachable one as a
/// stale `$OZMA_SOCK` and moving on.
///
/// `NotFound` (the socket file is gone) and `ConnectionRefused` (the file exists
/// but its ozmux has exited) mean a stale candidate — skip it. Any other IO error
/// is surfaced as [`OzmaError::Io`]. When every candidate is stale, returns
/// [`OzmaError::SocketUnavailable`] for the first (most-authoritative) one so the
/// message points at the socket the caller most expected to reach.
fn connect_first_reachable(candidates: &[String]) -> OzmaResult<UnixStream> {
    let mut stale: Option<(String, std::io::Error)> = None;
    for sock in candidates {
        match UnixStream::connect(sock) {
            Ok(stream) => return Ok(stream),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
                if stale.is_none() {
                    stale = Some((sock.clone(), e));
                }
            }
            Err(e) => return Err(OzmaError::Io(e)),
        }
    }
    let (path, cause) = stale.expect("candidates is non-empty and every entry was stale");
    Err(OzmaError::SocketUnavailable { path, cause })
}

/// Emits CUP + mount for new/changed placements and unmount for vanished
/// handles, updating `state` to the new frame. Degenerate rects are skipped.
fn flush_placements(
    out: &mut impl Write,
    state: &mut FlushState,
    placements: &[Placement],
) -> OzmaResult<()> {
    let mut current: HashMap<String, Rect> = HashMap::new();
    for p in placements {
        // Skip degenerate rects and invalid handles so a single bad placement
        // can't abort the whole flush (which would also desync flush state for
        // every later placement). An invalid handle never came from a minted
        // WebviewHandle, so it can never mount.
        if p.area.width == 0 || p.area.height == 0 || !valid_handle(&p.handle) {
            continue;
        }
        let (rows, cols) = clamp_dims(p.area.height, p.area.width);
        let key = Rect {
            x: p.area.x,
            y: p.area.y,
            width: cols,
            height: rows,
        };
        current.insert(p.handle.clone(), key);
        if state.last.get(&p.handle) != Some(&key) {
            let seq = mount(&p.handle, rows, cols)?;
            write!(out, "{}{}", cursor_to(p.area.y, p.area.x), seq)?;
        }
    }
    for handle in state.last.keys() {
        if !current.contains_key(handle) {
            write!(out, "{}", unmount(handle))?;
        }
    }
    out.flush()?;
    state.last = current;
    Ok(())
}

/// Emits the control-plane focus op (`ClientMsg::Focus`) when the focused handle
/// changed from the last flush. `Some(h)` focuses handle `h`; `None` blurs. No
/// write when unchanged (diff-driven, like geometry in `flush_placements`).
fn flush_focus(
    out: &mut impl Write,
    last_focused: &mut Option<String>,
    focused: &Option<String>,
) -> OzmaResult<()> {
    if last_focused == focused {
        return Ok(());
    }
    let line = serde_json::to_string(&ClientMsg::Focus {
        handle: focused.clone(),
        instance: None,
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
    pending: PendingRegisters,
    pending_compositing: PendingCompositing,
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
                    && let Some(handle) = v["handle"].as_str()
                    && let Some(active) = v["active"].as_bool()
                    && let Ok(mut map) = pending_compositing.lock()
                {
                    map.insert(handle.to_owned(), active);
                }
            } else if let Ok(reply) = serde_json::from_str::<RegisterReply>(trimmed)
                && let Some(reg) = pending.lock().ok().and_then(|mut q| q.pop_front())
            {
                let outcome = if reply.ok {
                    match reply.handle {
                        // Install handlers under the minted handle on this thread,
                        // before the next line is read, so a `call` pipelined right
                        // after the reply finds its handlers rather than racing the
                        // registrant's main thread.
                        Some(h) => {
                            if let Ok(mut map) = handlers.lock() {
                                map.insert(h.clone(), reg.handlers);
                            }
                            Ok(h)
                        }
                        None => Err(OzmaError::Register {
                            reason: "missing handle".into(),
                        }),
                    }
                } else {
                    Err(OzmaError::Register {
                        reason: reply.error.unwrap_or_else(|| "unknown".into()),
                    })
                };
                let _ = reg.reply.send(outcome);
            }
        }
        // The socket closed: drop every pending sender so any in-flight
        // register() waiter returns OzmaError::Disconnected instead of blocking
        // forever on a reply that will never arrive.
        if let Ok(mut q) = pending.lock() {
            q.clear();
        }
        disconnected.store(true, Ordering::Relaxed);
    });
}

fn dispatch_call(writer: &SharedWriter, handlers: &HandlerRegistry, call: IncomingCall) {
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
    pending: &PendingRegisters,
    pending_compositing: &PendingCompositing,
    disconnected: &Arc<AtomicBool>,
    generation: &Arc<AtomicU64>,
    registrations: &Arc<Mutex<Vec<Registration>>>,
    token: &str,
) {
    let candidates = resolve_ozma_sock_candidates();
    if candidates.is_empty() {
        tracing::debug!("reconnect: no OZMA_SOCK candidates, will retry on next signal");
        return;
    }
    let new_stream = match connect_first_reachable(&candidates) {
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
        disconnected.clone(),
    );
    let Ok(regs) = registrations.lock() else {
        return;
    };
    let Ok(mut w) = writer.lock() else { return };
    for reg in regs.iter() {
        let (tx, rx) = bounded::<OzmaResult<String>>(1);
        {
            let Ok(mut pq) = pending.lock() else {
                disconnected.store(true, Ordering::Relaxed);
                return;
            };
            pq.push_back(PendingRegister {
                reply: tx,
                handlers: reg.handlers.clone(),
            });
        }
        let line = match serde_json::to_string(&ClientMsg::Register(reg.kind.clone())) {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!("reconnect: serialize register failed: {e}");
                disconnected.store(true, Ordering::Relaxed);
                return;
            }
        };
        if writeln!(w, "{line}").and_then(|()| w.flush()).is_err() {
            tracing::debug!("reconnect: register write failed");
            disconnected.store(true, Ordering::Relaxed);
            return;
        }
        // NOTE: release writer lock while awaiting the reader-thread reply so
        // the reader can write focus/emit responses without deadlocking.
        drop(w);
        let new_handle = match rx.recv().unwrap_or(Err(OzmaError::Disconnected)) {
            Ok(h) => h,
            Err(e) => {
                tracing::debug!("reconnect: re-registration failed: {e}");
                disconnected.store(true, Ordering::Relaxed);
                return;
            }
        };
        let old = reg
            .handle_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Ok(mut map) = handlers.lock() {
            map.remove(&old);
            map.insert(new_handle.clone(), reg.handlers.clone());
        }
        if let Ok(mut slot) = reg.handle_slot.lock() {
            *slot = new_handle;
        }
        w = match writer.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
    }
    drop(w);
    generation.fetch_add(1, Ordering::Relaxed);
    tracing::debug!("reconnect: completed successfully");
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn pane_identity_prefers_ozmux_token() {
        assert_eq!(
            pane_identity(Some("tok".into()), Some("%3".into())),
            Some("tok".into())
        );
    }

    #[test]
    fn pane_identity_falls_back_to_tmux_pane() {
        assert_eq!(pane_identity(None, Some("%3".into())), Some("%3".into()));
    }

    #[test]
    fn pane_identity_treats_empty_token_as_absent() {
        assert_eq!(
            pane_identity(Some(String::new()), Some("%3".into())),
            Some("%3".into())
        );
    }

    #[test]
    fn pane_identity_none_when_neither_set() {
        assert_eq!(pane_identity(None, None), None);
    }

    #[test]
    fn socket_from_tmux_takes_first_comma_field() {
        assert_eq!(
            socket_from_tmux("/tmp/tmux-501/default,12345,0"),
            Some("/tmp/tmux-501/default")
        );
    }

    #[test]
    fn socket_from_tmux_handles_path_without_commas() {
        assert_eq!(
            socket_from_tmux("/tmp/only-socket"),
            Some("/tmp/only-socket")
        );
    }

    #[test]
    fn socket_from_tmux_none_for_empty() {
        assert_eq!(socket_from_tmux(""), None);
    }

    #[test]
    fn socket_from_tmux_none_for_leading_comma() {
        assert_eq!(socket_from_tmux(",12345,0"), None);
    }

    #[test]
    fn parse_show_environment_reads_value() {
        assert_eq!(
            parse_show_environment("OZMA_SOCK=/tmp/ctl.sock\n", "OZMA_SOCK"),
            Some("/tmp/ctl.sock")
        );
    }

    #[test]
    fn parse_show_environment_none_for_unset_marker() {
        assert_eq!(parse_show_environment("-OZMA_SOCK\n", "OZMA_SOCK"), None);
    }

    #[test]
    fn parse_show_environment_finds_key_among_many_lines() {
        let output = "FOO=bar\nOZMA_SOCK=/run/ozma/x.sock\nBAZ=qux\n";
        assert_eq!(
            parse_show_environment(output, "OZMA_SOCK"),
            Some("/run/ozma/x.sock")
        );
    }

    #[test]
    fn parse_show_environment_keeps_equals_in_value() {
        assert_eq!(
            parse_show_environment("OZMA_SOCK=/a=b/ctl.sock\n", "OZMA_SOCK"),
            Some("/a=b/ctl.sock")
        );
    }

    #[test]
    fn parse_show_environment_none_when_key_absent() {
        assert_eq!(parse_show_environment("OTHER=/x\n", "OZMA_SOCK"), None);
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

    #[test]
    fn flush_emits_mount_then_skips_unchanged() {
        let mut state = FlushState::default();
        let mut placements = vec![Placement {
            handle: "h1".into(),
            area: rect(2, 3, 48, 12),
        }];

        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &placements).unwrap();
        let first = String::from_utf8(buf).unwrap();
        assert!(first.contains("\x1b[4;3H"));
        assert!(first.contains("mount;h1;12;48"));

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
                .contains("mount;h1;12;50")
        );
    }

    #[test]
    fn flush_unmounts_vanished_handle() {
        let mut state = FlushState::default();
        let placements = vec![Placement {
            handle: "h1".into(),
            area: rect(0, 0, 10, 5),
        }];
        flush_placements(&mut Vec::new(), &mut state, &placements).unwrap();

        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &[]).unwrap();
        assert!(
            String::from_utf8(buf)
                .unwrap()
                .contains("unmount;h1")
        );
    }

    #[test]
    fn flush_skips_degenerate_area() {
        let mut state = FlushState::default();
        let placements = vec![Placement {
            handle: "h1".into(),
            area: rect(0, 0, 0, 5),
        }];
        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &placements).unwrap();
        assert!(String::from_utf8(buf).unwrap().is_empty());
    }

    #[test]
    fn flush_focus_emits_on_change_and_skips_unchanged() {
        let mut last = None;
        let mut buf = Vec::new();
        flush_focus(&mut buf, &mut last, &Some("v".to_string())).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(String::from_utf8(buf).unwrap().trim()).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], "v");

        let mut buf2 = Vec::new();
        flush_focus(&mut buf2, &mut last, &Some("v".to_string())).unwrap();
        assert!(
            String::from_utf8(buf2).unwrap().is_empty(),
            "unchanged focus emits nothing"
        );
    }

    #[test]
    fn flush_focus_emits_blur_on_none() {
        let mut last = Some("v".to_string());
        let mut buf = Vec::new();
        flush_focus(&mut buf, &mut last, &None).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(String::from_utf8(buf).unwrap().trim()).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], serde_json::Value::Null);
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

    #[test]
    fn reader_thread_inserts_compositing_into_shared_map() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let pending_compositing: Arc<Mutex<HashMap<String, bool>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingRegisters = Arc::new(Mutex::new(VecDeque::new()));

        let client = UnixStream::connect(&sock_path).unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client.try_clone().unwrap()));
        let (server_conn, _) = listener.accept().unwrap();

        spawn_reader(
            client,
            writer.clone(),
            handlers,
            pending,
            pending_compositing.clone(),
            Arc::new(AtomicBool::new(false)),
        );

        use std::io::Write;
        let mut server = server_conn;
        writeln!(
            server,
            r#"{{"op":"compositing","handle":"h1","active":true}}"#
        )
        .unwrap();
        server.flush().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        let map = pending_compositing.lock().unwrap();
        assert_eq!(map.get("h1"), Some(&true));
    }

    #[test]
    fn reader_thread_updates_compositing_to_false() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test2.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let pending_compositing: Arc<Mutex<HashMap<String, bool>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingRegisters = Arc::new(Mutex::new(VecDeque::new()));

        let client = UnixStream::connect(&sock_path).unwrap();
        let writer: SharedWriter = Arc::new(Mutex::new(client.try_clone().unwrap()));
        let (server_conn, _) = listener.accept().unwrap();

        spawn_reader(
            client,
            writer.clone(),
            handlers,
            pending,
            pending_compositing.clone(),
            Arc::new(AtomicBool::new(false)),
        );

        use std::io::Write;
        let mut server = server_conn;
        writeln!(
            server,
            r#"{{"op":"compositing","handle":"h1","active":false}}"#
        )
        .unwrap();
        server.flush().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        let map = pending_compositing.lock().unwrap();
        assert_eq!(map.get("h1"), Some(&false));
    }
}
