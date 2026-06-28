//! tmux window-bar interaction: click a window entry to `select-window`, and a
//! pointer cursor while hovering an entry.

use crate::input::InputPhase;
use crate::mode::tmux::TmuxActiveSet;
use crate::mode::tmux::window_bar::WindowEntry;
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use ozmux_tmux::{SelectWindow, TmuxClient};

/// Registers the window-bar click and hover-cursor systems.
pub(crate) struct WindowBarInputPlugin;

impl Plugin for WindowBarInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                switch_window_on_click.in_set(InputPhase::Dispatch),
                window_entry_hover_cursor.after(InputPhase::Hover),
            )
                .in_set(TmuxActiveSet),
        );
    }
}

/// Routes a press on a window entry to `select-window`: sends the tmux
/// `select-window -t @N` command for the pressed entry's window id.
fn switch_window_on_click(
    mut client: Option<Single<&mut TmuxClient>>,
    entries: Query<(&Interaction, &WindowEntry), Changed<Interaction>>,
) {
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    for (interaction, entry) in entries.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if let Err(e) = client.send(SelectWindow { id: entry.window }) {
            tracing::warn!(?e, window = entry.window.0, "select-window send failed");
        }
    }
}

/// Shows a pointer cursor while the mouse hovers any window entry, so entries
/// read as clickable. Runs after `InputPhase::Hover` so it wins over the
/// hyperlink system's baseline cursor write; leaving an entry reverts to that
/// baseline when the hyperlink system re-asserts.
fn window_entry_hover_cursor(
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    entries: Query<&Interaction, With<WindowEntry>>,
) {
    let hovering = entries
        .iter()
        .any(|i| matches!(i, Interaction::Hovered | Interaction::Pressed));
    if !hovering {
        return;
    }
    let Ok(mut icon) = cursor_icons.single_mut() else {
        return;
    };
    if !matches!(&*icon, CursorIcon::System(e) if *e == SystemCursorIcon::Pointer) {
        *icon = CursorIcon::System(SystemCursorIcon::Pointer);
    }
}
