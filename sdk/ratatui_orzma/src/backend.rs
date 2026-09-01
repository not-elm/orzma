//! A ratatui `Backend` wrapper that emits the webview APC verbs during `terminal.draw()`.

use crate::error::OrzmaError;
use crate::session::{FlushState, FramePlacements, Orzma, ReconnectHandle};
use crate::webview::SharedWriter;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use std::io::{self, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A ratatui [`Backend`] that wraps another backend and emits orzma webview
/// mount/unmount APC verbs (and the control-plane focus op) after each frame's cell
/// diff — so an app needs no separate post-draw flush call.
///
/// Construct it with [`OrzmaBackend::new`], passing the [`Orzma`] session it links
/// to, then build a normal ratatui terminal:
///
/// ```no_run
/// # use ratatui::Terminal;
/// # use ratatui::backend::CrosstermBackend;
/// # use ratatui_orzma::{Orzma, OrzmaBackend};
/// # use std::io::stdout;
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let orzma = Orzma::connect()?;
/// let backend = OrzmaBackend::new(CrosstermBackend::new(stdout()), &orzma);
/// let mut terminal = Terminal::new(backend)?;
/// # Ok(())
/// # }
/// ```
pub struct OrzmaBackend<B> {
    inner: B,
    frame: Arc<Mutex<FramePlacements>>,
    writer: SharedWriter,
    flush_state: FlushState,
    reconnect: ReconnectHandle,
    last_gen: u64,
    last_attempt: Option<Instant>,
    was_disconnected: bool,
}

impl<B> OrzmaBackend<B> {
    /// Wraps `inner`, linking it to `orzma`'s per-frame collector and control socket.
    pub fn new(inner: B, orzma: &Orzma) -> Self {
        Self {
            inner,
            frame: orzma.frame_handle(),
            writer: orzma.writer_handle(),
            flush_state: FlushState::default(),
            reconnect: orzma.reconnect_handle(),
            last_gen: 0,
            last_attempt: None,
            was_disconnected: false,
        }
    }
}

impl<B: Backend + Write> Backend for OrzmaBackend<B> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let current_gen = self.reconnect.generation.load(Ordering::Relaxed);
        let disconnected = self.reconnect.disconnected.load(Ordering::Relaxed);

        // NOTE: schedule the retry before anything below can return early. This
        // is the crate's only reconnect_tx sender, so a draw that bails out
        // first leaves the session with no way back and every later draw takes
        // the same path.
        if disconnected {
            let should_retry = self
                .last_attempt
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2));
            if should_retry {
                let _ = self.reconnect.reconnect_tx.try_send(());
                self.last_attempt = Some(Instant::now());
            }
        }

        // NOTE: a dropped connection has to reset as well. A replay that fails
        // part way refills only some id slots and never bumps the generation,
        // so gating the reset on the generation alone would keep diffing
        // against dead instance keys and never re-mount the placements.
        if current_gen != self.last_gen || (disconnected && !self.was_disconnected) {
            self.flush_state.reset();
            self.last_gen = current_gen;
        }
        self.was_disconnected = disconnected;

        self.inner.draw(content)?;

        let frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
        let flushed = if disconnected {
            self.flush_state.emit_placements(&mut self.inner, &frame)
        } else {
            self.flush_state
                .emit_frame(&mut self.inner, &self.writer, &frame)
        };
        drop(frame);
        flushed.map_err(to_io)?;

        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

fn to_io(e: OrzmaError) -> io::Error {
    io::Error::other(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    struct WritableTestBackend(ratatui::backend::TestBackend);

    impl io::Write for WritableTestBackend {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for WritableTestBackend {
        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            self.0.draw(content)
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            self.0.hide_cursor()
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            self.0.show_cursor()
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            self.0.get_cursor_position()
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.0.set_cursor_position(position)
        }

        fn clear(&mut self) -> io::Result<()> {
            self.0.clear()
        }

        fn size(&self) -> io::Result<Size> {
            self.0.size()
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            self.0.window_size()
        }

        fn flush(&mut self) -> io::Result<()> {
            Backend::flush(&mut self.0)
        }
    }

    /// Asserts that a draw taken while the control socket is down still
    /// succeeds and schedules a reconnect, even when a widget claims focus.
    ///
    /// Case: the user is typing in a focused webview when the orzma that owns
    /// the control socket goes away.
    #[test]
    fn a_disconnected_draw_with_a_focused_widget_still_schedules_a_reconnect() {
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Mutex};
        const INSTANCE: &str = "3f5a9c02d1e84b7690ab3cde12f45678";

        let frame = crate::session::FramePlacements::default();
        let frame = Arc::new(Mutex::new(frame));
        {
            let mut f = frame.lock().unwrap();
            f.record(INSTANCE.into(), ratatui::layout::Rect::new(0, 0, 10, 5));
            f.set_focused(INSTANCE.into());
        }

        // A socket whose peer is gone: every write to it fails, the way the
        // real one does once orzma has exited.
        let (near, far) = UnixStream::pair().unwrap();
        drop(far);

        let (tx, rx) = crossbeam_channel::bounded::<()>(1);
        let mut backend = OrzmaBackend {
            inner: WritableTestBackend(ratatui::backend::TestBackend::new(80, 24)),
            frame,
            writer: Arc::new(Mutex::new(near)),
            flush_state: FlushState::default(),
            reconnect: ReconnectHandle {
                disconnected: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                reconnect_tx: tx,
            },
            last_gen: 0,
            last_attempt: None,
            was_disconnected: false,
        };

        let no_cells: Vec<(u16, u16, &ratatui::buffer::Cell)> = Vec::new();
        let drawn = Backend::draw(&mut backend, no_cells.into_iter());

        assert!(
            drawn.is_ok(),
            "a dead control socket must not fail the draw: {:?}",
            drawn.err()
        );
        assert!(
            rx.try_recv().is_ok(),
            "the draw must have scheduled a reconnect"
        );
    }

    /// Asserts that a draw taken while the session is disconnected clears the
    /// flush state, without waiting for a generation bump.
    ///
    /// Case: a reconnect replay fails part way, so some of a registration's ids
    /// were re-minted and the generation never advanced.
    #[test]
    fn a_disconnected_draw_resets_flush_state() {
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Mutex};
        let disconnected = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (tx, _rx) = crossbeam_channel::bounded::<()>(1);
        let mut backend = OrzmaBackend {
            inner: WritableTestBackend(ratatui::backend::TestBackend::new(80, 24)),
            frame: Arc::new(Mutex::new(crate::session::FramePlacements::default())),
            writer: Arc::new(Mutex::new(UnixStream::pair().unwrap().0)),
            flush_state: FlushState::default(),
            reconnect: ReconnectHandle {
                disconnected,
                generation,
                reconnect_tx: tx,
            },
            last_gen: 0,
            last_attempt: None,
            was_disconnected: false,
        };
        backend
            .flush_state
            .last
            .insert("stale".into(), ratatui::layout::Rect::new(0, 0, 10, 5));

        let no_cells: Vec<(u16, u16, &ratatui::buffer::Cell)> = Vec::new();
        Backend::draw(&mut backend, no_cells.into_iter()).unwrap();

        assert!(
            backend.flush_state.last.is_empty(),
            "a disconnected draw must not keep diffing against dead instance keys"
        );
    }

    #[test]
    fn generation_change_resets_flush_state() {
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Mutex};
        let flush = FlushState::default();
        let disconnected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (tx, _rx) = crossbeam_channel::bounded::<()>(1);
        let reconnect = ReconnectHandle {
            disconnected: disconnected.clone(),
            generation: generation.clone(),
            reconnect_tx: tx,
        };
        let mut backend = OrzmaBackend {
            inner: WritableTestBackend(ratatui::backend::TestBackend::new(80, 24)),
            frame: Arc::new(Mutex::new(crate::session::FramePlacements::default())),
            writer: Arc::new(Mutex::new(UnixStream::pair().unwrap().0)),
            flush_state: flush,
            reconnect,
            last_gen: 0,
            last_attempt: None,
            was_disconnected: false,
        };
        backend
            .flush_state
            .last
            .insert("h1".into(), ratatui::layout::Rect::new(0, 0, 10, 5));
        generation.store(1, Ordering::Relaxed);
        use ratatui::backend::Backend;
        let no_cells: Vec<(u16, u16, &ratatui::buffer::Cell)> = Vec::new();
        Backend::draw(&mut backend, no_cells.into_iter()).unwrap();
        assert!(
            backend.flush_state.last.is_empty(),
            "flush_state should be reset after generation change"
        );
        assert_eq!(backend.last_gen, 1);
    }
}
