//! Bevy-free UDS wire protocol for ozmux: client↔daemon messages, a
//! length-prefixed codec, a passive `ClientMirror`, and a `Client` helper that
//! owns the connection state. (Daemon process + socket I/O + frame streaming are Plan 4.)

mod client;
mod codec;
mod message;
pub mod mirror;

pub use client::Client;
pub use codec::{MAX_MESSAGE_BYTES, PROTOCOL_VERSION, read_message, write_message};
pub use message::{ClientMessage, ServerMessage};
pub use mirror::ClientMirror;
