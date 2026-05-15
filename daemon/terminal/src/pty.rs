//! Low-level PTY plumbing: blocking reader thread, scrollback ring,
//! and the fixed-capacity byte ring buffer.
//!
//! No VT, no service orchestration. Higher-level wiring lives in
//! `crate::service`.

pub(crate) mod reader;
mod ring_buffer;
pub(crate) mod scrollback;
