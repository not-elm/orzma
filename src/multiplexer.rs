//! GUI-side multiplexer helpers: action dispatcher and layout-change
//! logging. The core ECS-native domain model lives in the
//! `ozmux_multiplexer` crate and is imported directly by consumers.

pub mod commands;
pub mod log;
