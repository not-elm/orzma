//! A ratatui `Backend` wrapper that emits webview OSC during `terminal.draw()`.

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
/// mount/unmount OSC (and the control-plane focus op) after each frame's cell
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
        }
    }
}

impl<B: Backend + Write> Backend for OrzmaBackend<B> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let current_gen = self.reconnect.generation.load(Ordering::Relaxed);
        if current_gen != self.last_gen {
            self.flush_state.reset();
            self.last_gen = current_gen;
        }

        self.inner.draw(content)?;

        let frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
        self.flush_state
            .emit_frame(&mut self.inner, &self.writer, &frame)
            .map_err(to_io)?;
        drop(frame);

        if self.reconnect.disconnected.load(Ordering::Relaxed) {
            let should_retry = self
                .last_attempt
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2));
            if should_retry {
                let _ = self.reconnect.reconnect_tx.try_send(());
                self.last_attempt = Some(Instant::now());
            }
        }

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
