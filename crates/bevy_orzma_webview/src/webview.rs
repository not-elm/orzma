//! In-process webviews anchored to terminal cells: CEF render wiring, the
//! `window.orzma` back-channel, APC mount and unmount, placement in the
//! terminal flow, and the CPU paint bridge.

pub(crate) mod apc;
pub(crate) mod mount;
pub(crate) mod paint;
pub(crate) mod render;
