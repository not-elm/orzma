//! Standalone status-bar rebuild system. Rebuilds when the set of Session
//! entities changes (Added/RemovedComponents on SessionMarker) or the
//! AttachedSession marker moves. Does NOT depend on per-session epoch
//! bumps — content changes (split / surface-add) do not redraw the
//! status bar.

use crate::font::TerminalUiFont;
use crate::ui::UiRoot;
use crate::ui::status_bar::build_status_bar;
use bevy::prelude::*;
use ozmux_multiplexer::{AttachedSession, SessionCreatedAt, SessionMarker};

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
    sessions: Query<
        (
            Entity,
            &Name,
            Has<AttachedSession>,
            Option<&SessionCreatedAt>,
        ),
        With<SessionMarker>,
    >,
    ui_root: Query<Entity, With<UiRoot>>,
    status_bar: Query<Entity, With<StatusBarRoot>>,
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

    let Ok(ui_root) = ui_root.single() else {
        return;
    };

    for e in status_bar.iter() {
        commands.entity(e).try_despawn();
    }

    // Sort by `SessionCreatedAt` (monotonic from `SessionNameCounter`)
    // rather than by `Entity`: Bevy's entity allocator does not guarantee
    // strictly monotonic indices across multiple deferred command queues,
    // so an Entity-based sort would not match session creation order.
    // Externally-spawned sessions without `SessionCreatedAt` sort last via
    // the `u32::MAX` fallback.
    let mut sessions: Vec<(Entity, String, bool, u32)> = sessions
        .iter()
        .map(|(e, name, attached, created)| {
            (
                e,
                name.as_str().to_string(),
                attached,
                created.map(|c| c.0).unwrap_or(u32::MAX),
            )
        })
        .collect();
    sessions.sort_by_key(|(_, _, _, created_at)| *created_at);
    let sessions: Vec<(Entity, String, bool)> = sessions
        .into_iter()
        .map(|(e, name, attached, _)| (e, name, attached))
        .collect();

    let ui_font_handle = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();
    build_status_bar(&mut commands, ui_root, &sessions, &ui_font_handle);
}
