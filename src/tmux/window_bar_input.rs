//! tmux window-bar interaction: click a window entry to `select-window`, and a
//! pointer cursor while hovering an entry.

use super::window_bar::WindowEntry;
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use ozmux_tmux::{TmuxConnection, select_window_command};

/// Routes a press on a window entry to `select-window`: sends the tmux
/// `select-window -t @N` command for the pressed entry's window id.
pub(super) fn switch_window_on_click(
    entries: Query<(&Interaction, &WindowEntry), Changed<Interaction>>,
    connection: NonSend<TmuxConnection>,
) {
    for (interaction, entry) in entries.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(client) = connection.client() else {
            continue;
        };
        let cmd = entry_command(entry);
        if let Err(e) = client.handle().send(&cmd) {
            tracing::warn!(?e, window = entry.window.0, "select-window send failed");
        }
    }
}

/// Shows a pointer cursor while the mouse hovers any window entry, so entries
/// read as clickable. Runs after `InputPhase::Hover` so it wins over the
/// hyperlink system's baseline cursor write; leaving an entry reverts to that
/// baseline when the hyperlink system re-asserts.
pub(super) fn window_entry_hover_cursor(
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

fn entry_command(entry: &WindowEntry) -> String {
    select_window_command(entry.window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::WindowId;

    #[test]
    fn entry_press_maps_to_select_window() {
        assert_eq!(select_window_command(WindowId(2)), "select-window -t @2");
    }

    #[test]
    fn entry_command_delegates_to_select_window_command() {
        let entry = WindowEntry {
            index: 3,
            window: WindowId(5),
        };
        assert_eq!(entry_command(&entry), "select-window -t @5");
    }
}
