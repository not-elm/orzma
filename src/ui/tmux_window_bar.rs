//! The tmux window status bar: a bottom row showing the session name and the
//! window list (`<index>:<name>`), with the active window highlighted and each
//! entry clickable to `select-window`.

use crate::font::TerminalUiFont;
use crate::input::InputPhase;
use crate::theme;
use crate::ui::UiRoot;
use crate::ui::palette;
use crate::ui::tmux_window_bar_input::{switch_window_on_click, window_entry_hover_cursor};
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, UiRect, Val};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozmux_tmux::{ActiveWindow, TmuxSession, TmuxWindow, WindowId};

/// Marker on the tmux window bar root Node — the fixed-height Row mounted at the
/// bottom of `UiRoot`. `spawn_window_bar` inserts it once; `rebuild_window_bar`
/// queries it to find the bar and despawn its children before rebuilding.
#[derive(Component)]
struct WindowBarRoot;

/// On a window-list entry button: records the tmux window the entry selects.
/// Read by the window-bar click handler (`switch_window_on_click`) to issue
/// `select-window`.
#[derive(Component)]
pub(crate) struct WindowEntry {
    /// tmux display index (`#{window_index}`), shown in the entry label.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "stored for future use; only .window is read now")
    )]
    pub(crate) index: u32,
    /// tmux window id (`@N`) the entry activates when clicked.
    pub(crate) window: WindowId,
}

/// On a window-list entry button: whether the entry's window is the session's
/// active window. Drives the active-vs-normal styling.
#[derive(Component)]
pub(crate) struct WindowEntryActive(
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "stored for future active-window styling; not yet read in production"
        )
    )]
    pub(crate) bool,
);

/// Marker on the session-name text node of the window bar.
#[derive(Component)]
struct SessionLabel;

/// Wires the tmux window status bar: spawns the bar on Startup and rebuilds its
/// children whenever the window set, active window, or session name changes.
pub struct OzmuxTmuxWindowBarPlugin;

impl Plugin for OzmuxTmuxWindowBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_window_bar);
        app.add_systems(Update, rebuild_window_bar.run_if(window_bar_dirty));
        app.add_systems(
            Update,
            (
                switch_window_on_click.in_set(InputPhase::Dispatch),
                window_entry_hover_cursor.after(InputPhase::Hover),
            ),
        );
    }
}

fn spawn_window_bar(
    mut commands: Commands,
    ui_root: Query<Entity, With<UiRoot>>,
    metrics: Option<Res<TerminalCellMetricsResource>>,
) {
    let Ok(ui_root) = ui_root.single() else {
        return;
    };
    let height = bar_height_px(metrics.as_deref());
    commands.spawn((
        Name::new("Tmux Window Bar"),
        Node {
            flex_direction: FlexDirection::Row,
            width: Val::Percent(100.0),
            height: Val::Px(height),
            align_items: AlignItems::Center,
            column_gap: Val::Px(theme::ELEMENT_PADDING_PX),
            padding: UiRect::axes(Val::Px(theme::ELEMENT_PADDING_PX), Val::Px(0.0)),
            ..default()
        },
        BackgroundColor(palette::PANEL),
        WindowBarRoot,
        ChildOf(ui_root),
    ));
}

/// True when the window set, any window's metadata, the active window, or the
/// session name may have changed this frame.
fn window_bar_dirty(
    mut removed_windows: RemovedComponents<TmuxWindow>,
    mut removed_active: RemovedComponents<ActiveWindow>,
    changed_windows: Query<(), Changed<TmuxWindow>>,
    added_active: Query<(), Added<ActiveWindow>>,
    changed_session: Query<(), Changed<TmuxSession>>,
) -> bool {
    !changed_windows.is_empty()
        || !added_active.is_empty()
        || !changed_session.is_empty()
        || removed_windows.read().next().is_some()
        || removed_active.read().next().is_some()
}

/// Despawns the window bar's children and rebuilds them from the window
/// entities + the session entity: a `[session]` label followed by one
/// clickable entry per window (ascending `index`), with the `ActiveWindow`
/// highlighted. Gated by `window_bar_dirty` at registration.
fn rebuild_window_bar(
    mut commands: Commands,
    bar: Query<Entity, With<WindowBarRoot>>,
    windows: Query<(&TmuxWindow, Has<ActiveWindow>)>,
    session: Query<&TmuxSession>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let Ok(bar) = bar.single() else {
        return;
    };
    commands.entity(bar).despawn_related::<Children>();

    let font = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();

    let session_name = session.iter().next().map(|s| s.name.as_str()).unwrap_or("");
    commands.spawn((
        SessionLabel,
        Text::new(format!("[{session_name}]")),
        TextColor(palette::ACCENT),
        TextFont {
            font: font.clone(),
            font_size: theme::UI_FONT_SIZE,
            ..default()
        },
        ChildOf(bar),
    ));

    let mut entries: Vec<(u32, WindowId, String, bool)> = windows
        .iter()
        .map(|(w, active)| (w.index, w.id, w.name.clone(), active))
        .collect();
    entries.sort_by_key(|(index, id, _, _)| (*index, id.0));

    for (index, id, name, active) in entries {
        let (bg, fg) = if active {
            (palette::TAB_ACTIVE_BG, palette::FOREGROUND)
        } else {
            (palette::PANEL, palette::MUTED)
        };
        let entry = commands
            .spawn((
                Button,
                Node {
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(theme::TAB_PADDING_X_PX), Val::Px(0.0)),
                    ..default()
                },
                BackgroundColor(bg),
                WindowEntry { index, window: id },
                WindowEntryActive(active),
                ChildOf(bar),
            ))
            .id();
        commands.spawn((
            Text::new(window_label(index, &name)),
            TextColor(fg),
            TextFont {
                font: font.clone(),
                font_size: theme::UI_FONT_SIZE,
                ..default()
            },
            ChildOf(entry),
        ));
    }
}

fn bar_height_px(metrics: Option<&TerminalCellMetricsResource>) -> f32 {
    metrics
        .map(|m| m.metrics.line_height_phys.floor().max(1.0))
        .unwrap_or(theme::UI_FONT_SIZE + 4.0)
}

fn window_label(index: u32, name: &str) -> String {
    format!("{index}:{name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};

    #[test]
    fn window_label_formats_index_and_name() {
        assert_eq!(window_label(0, "zsh"), "0:zsh");
        assert_eq!(window_label(12, ""), "12:");
    }

    fn metrics_fixture() -> TerminalCellMetricsResource {
        TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        }
    }

    #[test]
    fn rebuild_renders_window_entries_with_active_highlight() {
        use ozmux_tmux::{ActiveWindow, TmuxWindow};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(OzmuxTmuxWindowBarPlugin);
        app.insert_resource(metrics_fixture());
        app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
        app.world_mut().spawn((Node::default(), UiRoot));

        app.world_mut().spawn(TmuxWindow {
            id: WindowId(1),
            index: 0,
            name: "zsh".into(),
        });
        app.world_mut().spawn((
            TmuxWindow {
                id: WindowId(2),
                index: 1,
                name: "vim".into(),
            },
            ActiveWindow,
        ));

        app.update();
        app.update();

        let world = app.world_mut();
        let mut q = world.query::<(&WindowEntry, &WindowEntryActive)>();
        let mut entries: Vec<(u32, u32, bool)> = q
            .iter(world)
            .map(|(e, a)| (e.index, e.window.0, a.0))
            .collect();
        entries.sort();
        assert_eq!(entries, vec![(0, 1, false), (1, 2, true)]);
    }
}
