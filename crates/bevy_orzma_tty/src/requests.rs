//! Inbound request `EntityEvent`s: commands the host UI fires AT a
//! terminal entity, as opposed to the outbound `Tty*Signal`s in
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

pub use key_input::RequestTtyKeyInput;
pub use mouse_input::RequestTtyMouseInput;
pub use paste::RequestTtyPaste;
pub use resize::RequestTtyResize;
pub use scroll::RequestTtyScroll;
pub use selection::{
    CellSide, GridPoint, RequestTtySelectionClear, RequestTtySelectionKindChange,
    RequestTtySelectionStart, RequestTtySelectionStartAtViCursor, RequestTtySelectionUpdate,
    SelectionKind,
};
pub use vi_mode::{RequestTtyViMode, ViModeSwitch};
pub use vi_motion::{RequestTtyViMotion, ViMotion};

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
