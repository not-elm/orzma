//! Bevy-free terminal VT core. The serde DTO schema (`frame`, `color::RgbaColor`)
//! is always available; the VT engine (alacritty-backed) lives behind the
//! default-on `engine` feature, so wire-only consumers (`ozmux_proto`) can depend
//! on the DTOs with `default-features = false`.

#[cfg(feature = "engine")]
pub mod coalescer;
pub mod color;
#[cfg(feature = "engine")]
pub mod event;
pub mod frame;
#[cfg(feature = "engine")]
pub mod input;
#[cfg(feature = "engine")]
pub mod mouse;
#[cfg(feature = "engine")]
pub mod osc7;
#[cfg(feature = "pty")]
pub mod pty;
#[cfg(feature = "engine")]
pub mod vt;
