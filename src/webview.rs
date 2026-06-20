//! In-process webview feature: CEF render wiring and the window.ozma Tier 1
//! back-channel (render), OSC mount/unmount of inline webviews (osc), and
//! inline webviews rendered into the terminal text flow (inline).

pub(crate) mod inline;
pub(crate) mod osc;
pub(crate) mod render;
