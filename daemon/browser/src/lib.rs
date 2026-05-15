//! Headless browser service for ozmux. Owns one shared Chromium process,
//! one CDP page per Activity, and a `watch` channel of screencast snapshots
//! per Activity.
//!
//! This crate is in early scaffolding — see
//! `docs/superpowers/plans/2026-05-16-browser-activity.md` Phase 2 for the
//! task breakdown. The public API surface listed below is intentionally
//! sparse for Task 2.1; subsequent tasks fill it in.

pub mod bridge;
pub mod cookie;
pub mod error;
pub mod input;
pub mod page;
pub mod service;
pub mod snapshot;
pub mod state;
pub mod wire;

pub use error::{BrowserError, BrowserResult};
pub use snapshot::{BrowserSnapshot, NavState, ScreencastFrame};
pub use wire::{BrowserClientMsg, BrowserServerMsg, KeyKind, MouseButton, MouseKind, NavCommand};
