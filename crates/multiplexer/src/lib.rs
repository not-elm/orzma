//! ECS-native multiplexer for ozmux. Session, Pane, and Activity are Bevy
//! entities related by `ChildOf`. All mutations route through the
//! `MultiplexerCommands` SystemParam; the only observers handle dangling
//! `Entity` references when a child entity is despawned.
//!
//! No typed IDs (`SessionId` / `PaneId` / `ActivityId`) — every reference
//! is a Bevy `Entity`. Each entity also carries `Name` (from
//! `bevy::prelude::Name`) for tracing readability.

pub mod error;

pub use error::{MultiplexerError, MultiplexerResult};
