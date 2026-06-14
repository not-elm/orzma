//! I/O transport: owns a `tmux -CC` process and pumps its output through a
//! [`crate::ProtocolClient`].
