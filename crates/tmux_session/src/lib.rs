//! ozmux ⇄ tmux control-mode integration: owns a `tmux -CC` connection,
//! drains its transport events into the Bevy world, and tracks the
//! connection lifecycle. Phase 0 wires the plumbing and logs events; it
//! does not project entities or auto-connect.

mod connection;
mod event_pump;
mod plugin;
mod state;
