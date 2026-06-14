//! The Ozma session: socket connection, reader thread, flush.

use crate::error::OzmaError;
use crate::handler::BoxedHandler;
use crate::osc::{clamp_dims, cursor_to, mount_inline, unmount_inline};
use crate::protocol::{ClientMsg, IncomingCall, RegisterReply};
use crate::webview::{SharedWriter, Webview, WebviewHandle};
use crossbeam_channel::{bounded, Sender};
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use ratatui::Terminal;
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
}

impl FramePlacements {
    pub(crate) fn record(&mut self, handle: String, area: Rect) {
        self.placements.push(Placement { handle, area });
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
type PendingRegisters = Arc<Mutex<VecDeque<Sender<Result<String, OzmaError>>>>>;

/// An ozmux session: owns the control-socket connection and reader thread.
pub struct Ozma {
    writer: SharedWriter,
    handlers: HandlerRegistry,
    pending: PendingRegisters,
    frame: FramePlacements,
    flush_state: FlushState,
}

impl Ozma {
    /// Connects to `$OZMUX_SOCK`, performs the `hello` handshake, and spawns the
    /// background reader thread.
    pub fn connect() -> Result<Self, OzmaError> {
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
            handlers,
            pending,
            frame: FramePlacements::default(),
            flush_state: FlushState::default(),
        })
    }

    /// Registers a webview, blocking until the control plane mints its handle.
    pub fn register(&self, webview: Webview) -> Result<WebviewHandle, OzmaError> {
        let (tx, rx) = bounded(1);
        let line = serde_json::to_string(&ClientMsg::Register(webview.kind))?;
        {
            let mut w = self.writer.lock()?;
            // NOTE: push the pending sender while holding the writer lock so the
            // FIFO order matches the on-wire order — register replies are untagged,
            // so concurrent registrants would otherwise mismatch their handles.
            self.pending.lock()?.push_back(tx);
            writeln!(w, "{line}")?;
            w.flush()?;
        }

        let handle = rx.recv().map_err(|_| OzmaError::Disconnected)??;
        self.handlers
            .lock()?
            .insert(handle.clone(), Arc::new(webview.handlers));
        Ok(WebviewHandle::new(handle, self.writer.clone()))
    }

    /// Returns the per-frame placement collector, cleared, for `render_stateful_widget`.
    pub fn frame(&mut self) -> &mut FramePlacements {
        self.frame.placements.clear();
        &mut self.frame
    }

    /// Emits mount/unmount OSC for this frame's placements, after `terminal.draw()`.
    pub fn flush<B: Backend + Write>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<(), OzmaError> {
        let placements = std::mem::take(&mut self.frame.placements);
        let result = flush_placements(terminal.backend_mut(), &mut self.flush_state, &placements);
        self.frame.placements = placements;
        result
    }
}

/// Emits CUP + mount-inline for new/changed placements and unmount for vanished
/// handles, updating `state` to the new frame. Degenerate rects are skipped.
pub(crate) fn flush_placements(
    out: &mut impl Write,
    state: &mut FlushState,
    placements: &[Placement],
) -> Result<(), OzmaError> {
    let mut current: HashMap<String, (u16, u16, u16, u16)> = HashMap::new();
    for p in placements {
        if p.area.width == 0 || p.area.height == 0 {
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
                && let Some(tx) = pending.lock().ok().and_then(|mut q| q.pop_front())
            {
                let outcome = if reply.ok {
                    reply
                        .handle
                        .ok_or_else(|| OzmaError::Register { reason: "missing handle".into() })
                } else {
                    Err(OzmaError::Register {
                        reason: reply.error.unwrap_or_else(|| "unknown".into()),
                    })
                };
                let _ = tx.send(outcome);
            }
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
        Some(h) => h(call.args).map_err(|e| e.message().to_owned()),
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

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect { x, y, width: w, height: h }
    }

    #[test]
    fn flush_emits_mount_then_skips_unchanged() {
        let mut state = FlushState::default();
        let mut placements = vec![Placement { handle: "h1".into(), area: rect(2, 3, 48, 12) }];

        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &placements).unwrap();
        let first = String::from_utf8(buf).unwrap();
        assert!(first.contains("\x1b[4;3H"));
        assert!(first.contains("mount-inline;h1;12;48"));

        let mut buf2 = Vec::new();
        flush_placements(&mut buf2, &mut state, &placements).unwrap();
        assert!(String::from_utf8(buf2).unwrap().is_empty(), "unchanged frame emits nothing");

        placements[0].area = rect(2, 3, 50, 12);
        let mut buf3 = Vec::new();
        flush_placements(&mut buf3, &mut state, &placements).unwrap();
        assert!(String::from_utf8(buf3).unwrap().contains("mount-inline;h1;12;50"));
    }

    #[test]
    fn flush_unmounts_vanished_handle() {
        let mut state = FlushState::default();
        let placements = vec![Placement { handle: "h1".into(), area: rect(0, 0, 10, 5) }];
        flush_placements(&mut Vec::new(), &mut state, &placements).unwrap();

        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &[]).unwrap();
        assert!(String::from_utf8(buf).unwrap().contains("unmount-inline;h1"));
    }

    #[test]
    fn flush_skips_degenerate_area() {
        let mut state = FlushState::default();
        let placements = vec![Placement { handle: "h1".into(), area: rect(0, 0, 0, 5) }];
        let mut buf = Vec::new();
        flush_placements(&mut buf, &mut state, &placements).unwrap();
        assert!(String::from_utf8(buf).unwrap().is_empty());
    }
}
