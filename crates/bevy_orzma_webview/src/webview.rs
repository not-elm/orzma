//! In-process webview: CEF render wiring + window.orzma back-channel (render),
//! APC mount/unmount (apc), webviews rendered into the terminal flow (mount),
//! and the CPU paint bridge for platforms without a GPU paint path (paint).

pub(crate) mod apc;
pub(crate) mod mount;
pub(crate) mod paint;
pub(crate) mod render;
