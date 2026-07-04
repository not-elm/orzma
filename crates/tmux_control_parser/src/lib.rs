//! Parser for tmux control-mode (`tmux -CC`) output: a stateless per-line
//! parser plus a stateful block assembler.

pub use crate::assembler::{BlockAssembler, Frame};
pub use crate::error::{LayoutError, TmuxError, TmuxResult};
pub use crate::event::{ControlEvent, PaneId, SessionId, WindowId, unescape_capture};
pub use crate::layout::{Cell, CellDims, Divider, DividerAxis, SplitDir, WindowLayout, dividers};

pub mod assembler;
pub mod error;
pub mod event;
pub mod layout;
