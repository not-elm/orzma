//! In-process webviews anchored to terminal cells: CEF render wiring, the
//! `window.orzma` page bridge, the host-driven mount lifecycle, placement in
//! the terminal flow, and the CPU paint bridge.

pub(crate) mod assets;
pub(crate) mod forward_keys;
pub(crate) mod mount;
pub(crate) mod paint;
pub(crate) mod render;
pub(crate) mod scheme;
