//! The Ozma session: socket connection, reader thread, flush.

use crate::error::{OzmaError, OzmaResult};
use crate::handler::BoxedHandler;
use crate::osc::{clamp_dims, cursor_to, mount_inline, unmount_inline, valid_handle};
use crate::protocol::{ClientMsg, IncomingCall, RegisterReply};
use crate::webview::{SharedWriter, Webview, WebviewHandle};
use crossbeam_channel::{Sender, bounded};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
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
    natives: Vec<(String, Rect)>,
}

impl FramePlacements {
    pub(crate) fn record(&mut self, handle: String, area: Rect) {
        self.placements.push(Placement { handle, area });
    }

    /// Records a native widget's rect this frame (for spatial focus resolution).
    pub fn record_native(&mut self, id: String, area: Rect) {
        self.natives.push((id, area));
    }

    /// Returns the native widget rects recorded this frame.
    pub fn native_rects(&self) -> &[(String, Rect)] {
        &self.natives
    }

    #[cfg(test)]
    pub(crate) fn native_rects_for_test(&self) -> &[(String, Rect)] {
        &self.natives
    }

    #[cfg(test)]
    pub(crate) fn placements_for_test(&self) -> &[Placement] {
        &self.placements
    }
}

/// Last-emitted geometry per handle, for diff-driven flush.
#[derive(Debug, Default)]
pub(crate) struct FlushState {
    last: HashMap<String, (u16, u16, u16, u16)>,
}

type HandlerRegistry = Arc<Mutex<HashMap<String, Arc<HashMap<String, BoxedHandler>>>>>;
type PendingRegisters = Arc<Mutex<VecDeque<PendingRegister>>>;

/// One in-flight `register` awaiting its untagged reply: the oneshot to wake the
/// caller, plus the handlers to install once the control plane mints the handle.
struct PendingRegister {
    reply: Sender<OzmaResult<String>>,
    handlers: Arc<HashMap<String, BoxedHandler>>,
}

/// An ozmux session: owns the control-socket connection and reader thread.
pub struct Ozma {
    writer: SharedWriter,
    pending: PendingRegisters,
    frame: FramePlacements,
    flush_state: FlushState,
}

impl Ozma {
    /// Connects to `$OZMUX_SOCK`, performs the `hello` handshake, and spawns the
    /// background reader thread.
    pub fn connect() -> OzmaResult<Self> {
        let sock = std::env::var("OZMUX_SOCK").map_err(|_| OzmaError::NotInPane("OZMUX_SOCK"))?;
        let token =
            std::env::var("OZMUX_TOKEN").map_err(|_| OzmaError::NotInPane("OZMUX_TOKEN"))?;
        let stream = UnixStream::connect(sock)?;
        let writer: SharedWriter = Arc::new(Mutex::new(stream.try_clone()?));
        let handlers: HandlerRegistry = Arc::new(Mutex::new(HashMap::new()));
        let pending: PendingRegisters = Arc::new(Mutex::new(VecDeque::new()));

        {
            let line = serde_json::to_string(&ClientMsg::Hello { token })?;
            let mut w = writer.lock()?;
            writeln!(w, "{line}")?;
            w.flush()?;
        }

        spawn_reader(stream, writer.clone(), handlers.clone(), pending.clone());

        Ok(Self {
            writer,
            pending,
            frame: FramePlacements::default(),
            flush_state: FlushState::default(),
        })
    }

    /// Registers a webview, blocking until the control plane mints its handle.
    pub fn register(&self, webview: Webview) -> OzmaResult<WebviewHandle> {
        let Webview { kind, handlers } = webview;
        let (tx, rx) = bounded(1);
        let line = serde_json::to_string(&ClientMsg::Register(kind))?;
        {
            let mut w = self.writer.lock()?;
            // NOTE: push the pending entry while holding the writer lock so the
            // FIFO order matches the on-wire order — register replies are untagged,
            // so concurrent registrants would otherwise mismatch their handles.
            self.pending.lock()?.push_back(PendingRegister {
                reply: tx,
                handlers: Arc::new(handlers),
            });
            if let Err(e) = writeln!(w, "{line}").and_then(|()| w.flush()) {
                // The register never went out, so no reply will arrive for this
                // entry; drop it so it can't consume a later registrant's reply.
                self.pending.lock()?.pop_back();
                return Err(e.into());
            }
        }

        let handle = rx.recv().map_err(|_| OzmaError::Disconnected)??;
        Ok(WebviewHandle::new(handle, self.writer.clone()))
    }

    /// Returns the per-frame placement collector, cleared, for `render_stateful_widget`.
    pub fn frame(&mut self) -> &mut FramePlacements {
        self.frame.placements.clear();
        self.frame.natives.clear();
        &mut self.frame
    }

    /// Emits mount/unmount OSC for this frame's placements, after `terminal.draw()`.
    pub fn flush<B: Backend + Write>(&mut self, terminal: &mut Terminal<B>) -> OzmaResult<()> {
        let placements = std::mem::take(&mut self.frame.placements);
        let result = flush_placements(terminal.backend_mut(), &mut self.flush_state, &placements);
        self.frame.placements = placements;
        result
    }

    /// Clears the app-owned focus, blurring any focused webview back to the app.
    pub fn blur(&self) -> OzmaResult<()> {
        let line = serde_json::to_string(&ClientMsg::Focus {
            handle: None,
            instance: None,
        })?;
        let mut w = self.writer.lock()?;
        writeln!(w, "{line}")?;
        w.flush()?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn from_writer_for_test(writer: SharedWriter) -> Self {
        Self {
            writer,
            pending: Arc::new(Mutex::new(VecDeque::new())),
            frame: FramePlacements::default(),
            flush_state: FlushState::default(),
        }
    }
}

/// Emits CUP + mount-inline for new/changed placements and unmount for vanished
/// handles, updating `state` to the new frame. Degenerate rects are skipped.
pub(crate) fn flush_placements(
    out: &mut impl Write,
    state: &mut FlushState,
    placements: &[Placement],
) -> OzmaResult<()> {
    let mut current: HashMap<String, (u16, u16, u16, u16)> = HashMap::new();
    for p in placements {
        // Skip degenerate rects and invalid handles so a single bad placement
        // can't abort the whole flush (which would also desync flush state for
        // every later placement). An invalid handle never came from a minted
        // WebviewHandle, so it can never mount.
        if p.area.width == 0 || p.area.height == 0 || !valid_handle(&p.handle) {
            continue;
        }
        let (rows, cols) = clamp_dims(p.area.height, p.area.width);
        let key = (p.area.y, p.area.x, rows, cols);
        current.insert(p.handle.clone(), key);
        if state.last.get(&p.handle) != Some(&key) {
            let seq = mount_inline(&p.handle, rows, cols)?;
            write!(out, "{}{}", cursor_to(p.area.y, p.area.x), seq)?;
        }
    }
    for handle in state.last.keys() {
        if !current.contains_key(handle) {
            write!(out, "{}", unmount_inline(handle))?;
        }
    }
    out.flush()?;
    state.last = current;
    Ok(())
}

fn spawn_reader(
    stream: UnixStream,
    writer: SharedWriter,
    handlers: HandlerRegistry,
    pending: PendingRegisters,
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
            let is_call = serde_json::from_str::<serde_json::Value>(trimmed)
                .ok()
                .map(|v| v["op"] == "call")
                .unwrap_or(false);
            if is_call {
                if let Ok(call) = serde_json::from_str::<IncomingCall>(trimmed) {
                    dispatch_call(&writer, &handlers, call);
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
        Some(h) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h(call.args))) {
            Ok(r) => r.map_err(|e| e.message().to_owned()),
            Err(_) => Err("handler panicked".to_owned()),
        },
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn record_native_collects_native_rects() {
        let mut frame = FramePlacements::default();
        frame.record_native("editor".into(), Rect { x: 1, y: 2, width: 3, height: 4 });
        let natives = frame.native_rects_for_test();
        assert_eq!(natives.len(), 1);
        assert_eq!(natives[0].0, "editor");
        assert_eq!(natives[0].1, Rect { x: 1, y: 2, width: 3, height: 4 });
    }

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
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
        assert!(first.contains("mount-inline;h1;12;48"));

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
                .contains("mount-inline;h1;12;50")
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
                .contains("unmount-inline;h1")
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
    fn blur_writes_focus_op_with_null_handle() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixStream;
        let (a, b) = UnixStream::pair().unwrap();
        let ozma = Ozma::from_writer_for_test(std::sync::Arc::new(std::sync::Mutex::new(a)));
        ozma.blur().unwrap();
        let mut line = String::new();
        BufReader::new(b).read_line(&mut line).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], serde_json::Value::Null);
    }
}
