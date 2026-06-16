//! The tmux window status bar: a bottom row showing the session name and the
//! window list (`<index>:<name>`), with the active window highlighted and each
//! entry clickable to `select-window`.

use crate::font::{PowerlineFont, TerminalUiFont};
use crate::input::InputPhase;
use crate::theme;
use crate::ui::UiRoot;
use crate::ui::palette;
use crate::ui::tmux_window_bar_input::{switch_window_on_click, window_entry_hover_cursor};
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, UiRect, Val};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozmux_tmux::{ActiveWindow, TmuxProjectionSet, TmuxSession, TmuxWindow, WindowFlags, WindowId};

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

/// Wires the tmux window status bar: spawns the bar once after `UiRoot` exists
/// and rebuilds its children whenever the window set, active window, or session
/// name changes.
pub struct OzmuxTmuxWindowBarPlugin;

impl Plugin for OzmuxTmuxWindowBarPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: PostStartup, not Startup. `UiRoot` is spawned by `spawn_root_ui`
        // (a different plugin) via deferred Commands in Startup; with no ordering
        // edge between the two systems Bevy inserts no sync point, so a Startup
        // `spawn_window_bar` queries `UiRoot` before that spawn flushes, finds
        // nothing, and silently bails — the bar never mounts. PostStartup runs
        // after Startup's command flush, when `UiRoot` is guaranteed to exist.
        app.add_systems(PostStartup, spawn_window_bar);
        // NOTE: after the projection drain. `window_bar_dirty` reads
        // `RemovedComponents<TmuxWindow>` — one-frame events cleared at frame end.
        // The drain despawns windows on close (`%window-close` /
        // `%unlinked-window-close`); if the rebuild's run condition evaluated
        // before the drain it would miss the removal (gone by the next frame) and
        // a killed window's tab would linger in the bar.
        app.add_systems(
            Update,
            rebuild_window_bar
                .run_if(window_bar_dirty)
                .after(TmuxProjectionSet),
        );
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
            column_gap: Val::ZERO,
            padding: UiRect::ZERO,
            ..default()
        },
        BackgroundColor(palette::PANEL),
        WindowBarRoot,
        ChildOf(ui_root),
    ));
}

/// True when the window set, any window's metadata or flags, the active window,
/// or the session name may have changed this frame.
fn window_bar_dirty(
    mut removed_windows: RemovedComponents<TmuxWindow>,
    mut removed_active: RemovedComponents<ActiveWindow>,
    changed_windows: Query<(), Changed<TmuxWindow>>,
    changed_flags: Query<(), Changed<WindowFlags>>,
    added_active: Query<(), Added<ActiveWindow>>,
    changed_session: Query<(), Changed<TmuxSession>>,
) -> bool {
    // NOTE: drain both RemovedComponents readers up front, not inside the `||`
    // chain — a short-circuit on an earlier `Changed`/`Added` term would leave
    // the one-frame removal events unread, so they would re-fire (a stale,
    // spurious rebuild) on the next frame.
    let window_removed = removed_windows.read().next().is_some();
    let active_removed = removed_active.read().next().is_some();
    !changed_windows.is_empty()
        || !changed_flags.is_empty()
        || !added_active.is_empty()
        || !changed_session.is_empty()
        || window_removed
        || active_removed
}

/// Despawns the window bar's children and rebuilds the powerline layout: a
/// slate session block, one entry per window (ascending `index`) as flat text,
/// and the active window as an accent chevron pill. Gated by `window_bar_dirty`.
fn rebuild_window_bar(
    mut commands: Commands,
    bar: Query<Entity, With<WindowBarRoot>>,
    windows: Query<(&TmuxWindow, Has<ActiveWindow>, Option<&WindowFlags>)>,
    session: Query<&TmuxSession>,
    ui_font: Option<Res<TerminalUiFont>>,
    powerline_font: Option<Res<PowerlineFont>>,
) {
    let Ok(bar) = bar.single() else {
        return;
    };
    commands.entity(bar).despawn_related::<Children>();

    let font = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();
    let pl_font = powerline_font
        .as_deref()
        .map(|f| f.0.clone())
        .unwrap_or_else(|| font.clone());

    let session_name = session.iter().next().map(|s| s.name.as_str()).unwrap_or("");
    build_session_block(&mut commands, bar, session_name, &font, &pl_font);

    let mut entries: Vec<(u32, WindowId, String, bool, WindowFlags)> = windows
        .iter()
        .map(|(w, active, flags)| {
            (
                w.index,
                w.id,
                w.name.clone(),
                active,
                flags.copied().unwrap_or_default(),
            )
        })
        .collect();
    entries.sort_by_key(|(index, id, _, _, _)| (*index, id.0));

    for (index, id, name, active, flags) in entries {
        build_window_entry(
            &mut commands,
            bar,
            index,
            id,
            &name,
            active,
            flags,
            &font,
            &pl_font,
        );
    }
}

/// Spawns the leading session block (slate fill + session name) and its
/// trailing powerline arrow (slate-colored, on the bar background).
fn build_session_block(
    commands: &mut Commands,
    bar: Entity,
    session_name: &str,
    font: &Handle<Font>,
    pl_font: &Handle<Font>,
) {
    let block = commands
        .spawn((
            SessionLabel,
            Node {
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(theme::TAB_PADDING_X_PX), Val::Px(0.0)),
                ..default()
            },
            BackgroundColor(palette::SESSION_BG),
            ChildOf(bar),
        ))
        .id();
    commands.spawn((
        Text::new(session_name.to_string()),
        TextColor(palette::FOREGROUND),
        TextFont {
            font: font.clone(),
            font_size: theme::UI_FONT_SIZE,
            ..default()
        },
        ChildOf(block),
    ));
    commands.spawn((
        Text::new(theme::POWERLINE_RIGHT.to_string()),
        TextColor(palette::SESSION_BG),
        TextFont {
            font: pl_font.clone(),
            font_size: theme::UI_FONT_SIZE,
            ..default()
        },
        ChildOf(bar),
    ));
}

/// Spawns one window entry as a `Button` carrying `WindowEntry`. The active
/// window is an accent chevron pill (left glyph + accent fill + right glyph,
/// glyphs as children so the whole pill is clickable); inactive windows are
/// flat text.
#[expect(
    clippy::too_many_arguments,
    reason = "spawn helper threads the per-entry inputs + both font handles"
)]
fn build_window_entry(
    commands: &mut Commands,
    bar: Entity,
    index: u32,
    id: WindowId,
    name: &str,
    active: bool,
    flags: WindowFlags,
    font: &Handle<Font>,
    pl_font: &Handle<Font>,
) {
    let (label_color, flag_color) = entry_colors(active, flags);
    let label = window_label(index, name);
    let suffix = flag_suffix(flags);

    let entry = commands
        .spawn((
            Name::new("Window Entry"),
            Button,
            Node {
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::NONE),
            WindowEntry { index, window: id },
            WindowEntryActive(active),
            ChildOf(bar),
        ))
        .id();

    if active {
        commands.spawn((
            Text::new(theme::POWERLINE_LEFT.to_string()),
            TextColor(palette::ACCENT),
            TextFont {
                font: pl_font.clone(),
                font_size: theme::UI_FONT_SIZE,
                ..default()
            },
            ChildOf(entry),
        ));
        let fill = commands
            .spawn((
                Node {
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(theme::TAB_PADDING_X_PX), Val::Px(0.0)),
                    ..default()
                },
                BackgroundColor(palette::ACCENT),
                ChildOf(entry),
            ))
            .id();
        spawn_entry_label(
            commands,
            fill,
            &label,
            &suffix,
            label_color,
            flag_color,
            font,
        );
        commands.spawn((
            Text::new(theme::POWERLINE_RIGHT.to_string()),
            TextColor(palette::ACCENT),
            TextFont {
                font: pl_font.clone(),
                font_size: theme::UI_FONT_SIZE,
                ..default()
            },
            ChildOf(entry),
        ));
    } else {
        let pad = commands
            .spawn((
                Node {
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(theme::TAB_PADDING_X_PX), Val::Px(0.0)),
                    ..default()
                },
                ChildOf(entry),
            ))
            .id();
        spawn_entry_label(
            commands,
            pad,
            &label,
            &suffix,
            label_color,
            flag_color,
            font,
        );
    }
}

/// Spawns the `<index>:<name>` label and, when non-empty, the flag suffix as a
/// second (differently colored) `Text` node under `parent`.
fn spawn_entry_label(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    suffix: &str,
    label_color: Color,
    flag_color: Color,
    font: &Handle<Font>,
) {
    commands.spawn((
        Text::new(label.to_string()),
        TextColor(label_color),
        TextFont {
            font: font.clone(),
            font_size: theme::UI_FONT_SIZE,
            ..default()
        },
        ChildOf(parent),
    ));
    if !suffix.is_empty() {
        commands.spawn((
            Text::new(suffix.to_string()),
            TextColor(flag_color),
            TextFont {
                font: font.clone(),
                font_size: theme::UI_FONT_SIZE,
                ..default()
            },
            ChildOf(parent),
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

/// The (label, flag-suffix) text color pair for a window entry. Active entries
/// use dark text on the accent fill; inactive entries are `MUTED`, with the
/// flag suffix tinted `FLAG_WARN` when a bell or activity flag is set.
fn entry_colors(is_active: bool, flags: WindowFlags) -> (Color, Color) {
    if is_active {
        (palette::BACKGROUND, palette::BACKGROUND)
    } else if flags.intersects(WindowFlags::BELL | WindowFlags::ACTIVITY) {
        (palette::MUTED, palette::FLAG_WARN)
    } else {
        (palette::MUTED, palette::MUTED)
    }
}

/// The flag suffix appended after a window's `<index>:<name>`, e.g. `" Z!"`.
/// Empty when no flags are set. `*` (current) and `-` (last) are not shown:
/// "current" is conveyed by the accent pill.
fn flag_suffix(flags: WindowFlags) -> String {
    let mut chars = String::new();
    if flags.contains(WindowFlags::ZOOM) {
        chars.push('Z');
    }
    if flags.contains(WindowFlags::BELL) {
        chars.push('!');
    }
    if flags.contains(WindowFlags::ACTIVITY) {
        chars.push('#');
    }
    if flags.contains(WindowFlags::SILENCE) {
        chars.push('~');
    }
    if flags.contains(WindowFlags::MARKED) {
        chars.push('M');
    }
    if chars.is_empty() {
        chars
    } else {
        format!(" {chars}")
    }
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
    fn bar_mounts_when_ui_root_is_spawned_in_startup() {
        // Regression: UiRoot is created by another Startup system via deferred
        // Commands. spawn_window_bar also ran in Startup, with no ordering edge,
        // so it queried UiRoot before that spawn flushed and silently bailed —
        // the bar never appeared. It must mount regardless of that race.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(metrics_fixture());
        app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn((Node::default(), UiRoot));
        });
        app.add_plugins(OzmuxTmuxWindowBarPlugin);

        app.update();

        let world = app.world_mut();
        let mut bar_q = world.query_filtered::<(), With<WindowBarRoot>>();
        assert_eq!(
            bar_q.iter(world).count(),
            1,
            "window bar root must mount even when UiRoot is spawned in Startup"
        );
    }

    #[test]
    fn flag_suffix_orders_and_space_prefixes() {
        use ozmux_tmux::WindowFlags;
        assert_eq!(flag_suffix(WindowFlags::default()), "");
        assert_eq!(flag_suffix(WindowFlags::ZOOM | WindowFlags::BELL), " Z!");
        assert_eq!(
            flag_suffix(WindowFlags::ACTIVITY | WindowFlags::SILENCE | WindowFlags::MARKED),
            " #~M"
        );
    }

    #[test]
    fn entry_colors_active_is_dark_text() {
        use ozmux_tmux::WindowFlags;
        assert_eq!(
            entry_colors(true, WindowFlags::BELL),
            (palette::BACKGROUND, palette::BACKGROUND)
        );
    }

    #[test]
    fn entry_colors_inactive_bell_or_activity_is_warn() {
        use ozmux_tmux::WindowFlags;
        assert_eq!(
            entry_colors(false, WindowFlags::BELL),
            (palette::MUTED, palette::FLAG_WARN)
        );
    }

    #[test]
    fn entry_colors_inactive_plain_is_muted() {
        use ozmux_tmux::WindowFlags;
        assert_eq!(
            entry_colors(false, WindowFlags::default()),
            (palette::MUTED, palette::MUTED)
        );
    }

    #[test]
    fn rebuild_renders_session_block_active_pill_and_flags() {
        use ozmux_tmux::{ActiveWindow, TmuxSession, TmuxWindow, WindowFlags};
        use tmux_control_parser::SessionId;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(OzmuxTmuxWindowBarPlugin);
        app.insert_resource(metrics_fixture());
        app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
        app.world_mut().spawn((Node::default(), UiRoot));
        app.world_mut().spawn(TmuxSession {
            id: SessionId(1),
            name: "main".into(),
        });
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
            WindowFlags::ZOOM,
        ));

        app.update();
        app.update();

        let world = app.world_mut();
        let mut session_q = world.query_filtered::<(), With<SessionLabel>>();
        assert_eq!(session_q.iter(world).count(), 1, "one session block");

        let mut text_q = world.query::<&Text>();
        let texts: Vec<String> = text_q.iter(world).map(|t| t.0.clone()).collect();
        assert!(texts.iter().any(|t| t == "main"), "session name: {texts:?}");
        assert!(
            texts.iter().any(|t| t == "1:vim"),
            "active label: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == theme::POWERLINE_LEFT),
            "left chevron: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == theme::POWERLINE_RIGHT),
            "right chevron: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == " Z"),
            "zoom flag suffix: {texts:?}"
        );
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
