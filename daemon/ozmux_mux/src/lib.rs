//! Bevy-free multiplexer core: the (future) source of truth for the
//! Session > Workspace > LayoutNode(Split | Pane) > Surface hierarchy.
//! Every mutation on `mux::Mux` returns a list of `event::MuxEvent`s;
//! the daemon serializes them to UDS and the Bevy mirror applies them.

pub mod error;
pub mod event;
pub mod geometry;
pub mod id;
pub mod mux;
pub mod surface;
pub mod tree;
