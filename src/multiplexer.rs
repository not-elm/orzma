//! GUI-side multiplexer helpers (action dispatcher + log system). The
//! core ECS-native domain model lives in the `ozmux_multiplexer` crate;
//! this module re-exports its public API at the old import paths for
//! transitional compatibility, and keeps the GUI-only systems
//! (action dispatch, layout-change logging) under their original paths.

pub mod commands;
pub mod log;

pub use ozmux_multiplexer::{
    ActiveActivity, ActivePane, ActivityKind, ActivityMarker, AttachedSession, BrowserProfile,
    CopyMode, LayoutCells, MultiplexerCommands, MultiplexerPlugin, PaneDimensions, PaneMarker,
    SessionDimensions, SessionMarker, SessionUiSubtree,
};
