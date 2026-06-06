//! Bevy-free UDS wire protocol for ozmux: client↔daemon messages, an NDJSON
//! codec, and a passive `ClientMirror`. (Daemon process + socket I/O + frame
//! streaming are Plan 4.)

mod codec;
mod message;

pub use codec::{MAX_LINE_BYTES, PROTOCOL_VERSION, read_message, write_message};
pub use message::{ClientMessage, ServerMessage};
