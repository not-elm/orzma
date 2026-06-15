//! A ratatui `Backend` wrapper that emits webview OSC during `terminal.draw()`.

use crate::error::OzmaError;
use crate::session::{FlushState, FramePlacements, Ozma};
use crate::webview::SharedWriter;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

/// A ratatui [`Backend`] that wraps another backend and emits ozmux webview
/// mount/unmount OSC (and the control-plane focus op) after each frame's cell
/// diff — so an app needs no separate post-draw flush call.
///
/// Construct it with [`OzmaBackend::new`], passing the [`Ozma`] session it links
/// to, then build a normal ratatui terminal:
///
/// ```no_run
/// # use ratatui::Terminal;
/// # use ratatui::backend::CrosstermBackend;
/// # use ratatui_ozma::{Ozma, OzmaBackend};
/// # use std::io::stdout;
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let ozma = Ozma::connect()?;
/// let backend = OzmaBackend::new(CrosstermBackend::new(stdout()), &ozma);
/// let mut terminal = Terminal::new(backend)?;
/// # Ok(())
/// # }
/// ```
pub struct OzmaBackend<B> {
    inner: B,
    frame: Arc<Mutex<FramePlacements>>,
    writer: SharedWriter,
    flush_state: FlushState,
}

impl<B> OzmaBackend<B> {
    /// Wraps `inner`, linking it to `ozma`'s per-frame collector and control socket.
    pub fn new(inner: B, ozma: &Ozma) -> Self {
        Self {
            inner,
            frame: ozma.frame_handle(),
            writer: ozma.writer_handle(),
            flush_state: FlushState::default(),
        }
    }
}

impl<B: Backend + Write> Backend for OzmaBackend<B> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)?;
        let frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
        self.flush_state
            .emit_frame(&mut self.inner, &self.writer, &frame)
            .map_err(to_io)
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

fn to_io(e: OzmaError) -> io::Error {
    io::Error::other(e)
}
