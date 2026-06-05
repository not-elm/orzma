//! Bevy-free terminal VT core. Provides wire DTO schema, color types,
//! VT submodules (coalescer, OSC 7, frame builders, and damage classification),
//! shared by both the daemon and the Bevy GUI.

pub mod coalescer;
pub mod color;
pub mod event;
pub mod frame;
pub mod input;
pub mod mouse;
pub mod osc7;
pub mod vt;
