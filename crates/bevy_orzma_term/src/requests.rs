//! Inbound request `EntityEvent`s: commands the host UI fires AT a
//! terminal entity, as opposed to the outbound `Term*Signal`s in
//! `signals.rs` that are drained FROM the VT.

use crate::requests::{
    key_input::KeyInputPlugin, mouse_input::MouseInputPlugin, paste::PastePlugin,
    resize::ResizePlugin, scroll::ScrollPlugin, selection::SelectionPlugin, vi_mode::ViModePlugin,
    vi_motion::ViMotionPlugin,
};
use bevy::prelude::*;

mod key_input;
mod mouse_input;
mod paste;
mod resize;
mod scroll;
mod selection;
mod vi_mode;
mod vi_motion;

pub use key_input::RequestTermKeyInput;
pub use mouse_input::RequestTermMouseInput;
pub use paste::RequestTermPaste;
pub use resize::RequestTermResize;
pub use scroll::{RequestTermScroll, ScrollKind};
pub use selection::{CellSide, RequestTermSelection, SelectionKind, SelectionOp};
pub use vi_mode::{RequestTermViMode, ViModeSwitch};
pub use vi_motion::{RequestTermViMotion, ViMotion};

pub(crate) struct OrzmaEventRequestPlugin;

impl Plugin for OrzmaEventRequestPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            KeyInputPlugin,
            MouseInputPlugin,
            PastePlugin,
            ResizePlugin,
            ScrollPlugin,
            SelectionPlugin,
            ViModePlugin,
            ViMotionPlugin,
        ));
    }
}
