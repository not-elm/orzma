//! Standalone status-bar rebuild system. Rebuilds when the set of Session
//! entities changes (Added/RemovedComponents on SessionMarker) or the
//! AttachedSession marker moves. Does NOT depend on per-session epoch
//! bumps — content changes (split / activity-add) do not redraw the
//! status bar.

use crate::font::TerminalUiFont;
use crate::ui::UiRoot;
use bevy::prelude::*;
use ozmux_multiplexer::{AttachedSession, SessionMarker};

/// Marker on the currently-active status bar root Node. `build_status_bar`
/// inserts this on the bar entity it spawns; the standalone rebuild
/// system queries this to find and despawn the previous bar before
/// spawning a replacement.
#[derive(Component)]
pub struct StatusBarRoot;

/// Despawns the existing `StatusBarRoot` and rebuilds via
/// `crate::ui::status_bar::build_status_bar` when:
/// - any `SessionMarker` was added or removed this frame, OR
/// - any `AttachedSession` marker was added or removed this frame.
pub fn rebuild_status_bar_on_session_set_change(
    mut commands: Commands,
    mut attached_removed: RemovedComponents<AttachedSession>,
    mut sessions_removed: RemovedComponents<SessionMarker>,
    sessions_q: Query<(Entity, &Name, Has<AttachedSession>), With<SessionMarker>>,
    ui_root_q: Query<Entity, With<UiRoot>>,
    status_bar_q: Query<Entity, With<StatusBarRoot>>,
    sessions_added: Query<(), Added<SessionMarker>>,
    attached_added: Query<(), Added<AttachedSession>>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let any_session_added = sessions_added.iter().count() > 0;
    let any_session_removed = sessions_removed.read().count() > 0;
    let any_attached_added = attached_added.iter().count() > 0;
    let any_attached_removed = attached_removed.read().count() > 0;

    if !(any_session_added || any_session_removed || any_attached_added || any_attached_removed) {
        return;
    }

    let Ok(ui_root) = ui_root_q.single() else {
        return;
    };

    for e in status_bar_q.iter() {
        commands.entity(e).try_despawn();
    }

    let mut sessions: Vec<(Entity, String, bool)> = sessions_q
        .iter()
        .map(|(e, name, attached)| (e, name.as_str().to_string(), attached))
        .collect();
    sessions.sort_by_key(|(e, _, _)| *e);

    let ui_font_handle = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();
    crate::ui::status_bar::build_status_bar(&mut commands, ui_root, &sessions, &ui_font_handle);
}
