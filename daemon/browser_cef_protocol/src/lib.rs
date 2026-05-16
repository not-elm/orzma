//! Shared protocol types between ozmux daemon (`daemon/browser`) and the
//! `cef_host` child process. Pure data — no runtime, no I/O.

pub mod bytes_serde;
pub mod types;
pub mod wire;
