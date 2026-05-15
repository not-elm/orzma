//! Public service surface (`BrowserService`). Mirrors `TerminalService` in
//! shape but with a `tokio::sync::watch` channel per activity instead of a
//! `broadcast` delta ring. Filled in by Task 2.8.

// Subsequent tasks (2.5–2.9) populate this module.
