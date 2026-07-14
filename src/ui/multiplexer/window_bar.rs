//! The multiplexer window bar: the row of window entries mounted under
//! `WindowBarContainer` (`crate::ui::multiplexer`). Ported from
//! `5cc2676^:src/ui/tmux/window_bar.rs`, dropping the tmux session block
//! (`build_session_block`/`SessionLabel`) and the per-window bell/activity
//! flags (`WindowFlags`/`flag_suffix`) — the multiplexer has neither a
//! session concept nor per-window flags yet.

use crate::font::TerminalUiFont;
use crate::multiplexer::request::{SelectWindowRequest, WindowSelect};
use crate::multiplexer::window::{ActiveMultiplexerWindow, MultiplexerWindow, active_marker_moved};
use crate::ui::multiplexer::WindowBarContainer;
use bevy::prelude::*;
use orzma_tty_engine::TerminalTitle;

/// Text color of the active window entry, read against `ACTIVE_BG`. The
/// theme module is gone (see `ui/multiplexer/rename_prompt.rs` /
/// `divider_handle.rs`); this file declares its own colors the same way.
const ACTIVE_FG: Color = Color::srgb(0.05, 0.06, 0.08);
/// Background fill of the active window entry: an accent tone distinguishing
/// it from inactive entries.
const ACTIVE_BG: Color = Color::srgb(0.40, 0.62, 0.95);
/// Text color of an inactive window entry.
const INACTIVE_FG: Color = Color::srgb(0.65, 0.65, 0.70);
/// Background fill of an inactive window entry: no fill, letting the bar's
/// own background show through.
const INACTIVE_BG: Color = Color::NONE;

/// Horizontal padding, in logical px, either side of an entry's label.
const ENTRY_PADDING_X_PX: f32 = 8.0;
/// Font size of a window entry's label.
const ENTRY_FONT_SIZE_PX: f32 = 12.0;

/// On a window-list entry `Button`: the window's display index, read by
/// `select_window_on_click` to fire `SelectWindowRequest(WindowSelect::Index(..))`.
#[derive(Component)]
struct WindowEntry {
    index: u32,
}

/// Wires the multiplexer window bar: rebuilds its entries whenever the window
/// set, active window, or an unnamed window's active-pane title changes
/// (`window_bar_dirty`), and turns a press on an entry into a
/// `SelectWindowRequest`.
pub(super) struct WindowBarPlugin;

impl Plugin for WindowBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SelectWindowRequest>().add_systems(
            Update,
            (
                rebuild_window_bar.run_if(window_bar_dirty),
                select_window_on_click,
            ),
        );
    }
}

/// True when the window set, any window's name, the active window, or an
/// unnamed window's active-pane title may have changed this frame.
fn window_bar_dirty(
    mut removed_windows: RemovedComponents<MultiplexerWindow>,
    mut removed_active: RemovedComponents<ActiveMultiplexerWindow>,
    changed_windows: Query<(), Changed<MultiplexerWindow>>,
    added_active: Query<(), Added<ActiveMultiplexerWindow>>,
    changed_titles: Query<(), Changed<TerminalTitle>>,
) -> bool {
    // NOTE: drain the RemovedComponents readers up front (active_marker_moved
    // drains its own), not inside the `||` chain — a short-circuit on an
    // earlier `Changed`/`Added` term would leave the one-frame removal events
    // unread, so they would re-fire (a stale, spurious rebuild) next frame.
    let marker_moved = active_marker_moved(&mut removed_active, &added_active);
    let window_removed = removed_windows.read().next().is_some();
    !changed_windows.is_empty() || !changed_titles.is_empty() || window_removed || marker_moved
}

/// Despawns the bar's current entries and respawns one per `MultiplexerWindow`
/// (ascending `index`), each a `Button` carrying `WindowEntry`. Gated by
/// `window_bar_dirty`, and a no-op when the rendered rows would be identical
/// to the existing ones — the dirty gate over-approximates (e.g. any pane's
/// title change fires it, but only unnamed windows' ACTIVE panes render), so
/// a busy shell churning its OSC title must not despawn/respawn the bar.
fn rebuild_window_bar(
    mut commands: Commands,
    bar: Query<Entity, With<WindowBarContainer>>,
    windows: Query<(&MultiplexerWindow, Has<ActiveMultiplexerWindow>)>,
    titles: Query<&TerminalTitle>,
    existing: Query<(&WindowEntry, &BackgroundColor, &Children)>,
    texts: Query<&Text>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let Ok(bar) = bar.single() else {
        return;
    };

    let mut entries: Vec<(&MultiplexerWindow, bool)> = windows.iter().collect();
    entries.sort_by_key(|(window, _)| window.index);
    let desired: Vec<(u32, String, Color)> = entries
        .iter()
        .map(|(window, active)| {
            let label = window_label(window.index, &display_name(window, &titles));
            (window.index, label, entry_colors(*active).1)
        })
        .collect();
    let mut current: Vec<(u32, String, Color)> = existing
        .iter()
        .map(|(entry, bg, children)| {
            let label = children
                .iter()
                .find_map(|child| texts.get(child).ok())
                .map(|text| text.0.clone())
                .unwrap_or_default();
            (entry.index, label, bg.0)
        })
        .collect();
    current.sort_by_key(|(index, _, _)| *index);
    if desired == current {
        return;
    }

    commands.entity(bar).despawn_related::<Children>();

    let ui = ui_font.as_deref().cloned().unwrap_or_default();
    for (window, active) in entries {
        let label = window_label(window.index, &display_name(window, &titles));
        let (fg, bg) = entry_colors(active);
        let entry = commands
            .spawn((
                Name::new("Window Entry"),
                Button,
                Node {
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(ENTRY_PADDING_X_PX), Val::ZERO),
                    ..default()
                },
                BackgroundColor(bg),
                WindowEntry {
                    index: window.index,
                },
                ChildOf(bar),
            ))
            .id();
        commands.spawn((
            Text::new(label),
            TextColor(fg),
            ui.text_font(FontSize::Px(ENTRY_FONT_SIZE_PX)),
            ChildOf(entry),
        ));
    }
}

/// Consumes a press on a window entry, firing
/// `SelectWindowRequest(WindowSelect::Index(..))` for the pressed entry's
/// window index.
fn select_window_on_click(
    mut requests: MessageWriter<SelectWindowRequest>,
    entries: Query<(&Interaction, &WindowEntry), Changed<Interaction>>,
) {
    for (interaction, entry) in entries.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        requests.write(SelectWindowRequest(WindowSelect::Index(entry.index)));
    }
}

/// The display name for a window entry: `window.name` if set, else the
/// active pane's `TerminalTitle`, falling back to an empty string when
/// neither is available.
fn display_name(window: &MultiplexerWindow, titles: &Query<&TerminalTitle>) -> String {
    if let Some(name) = &window.name {
        return name.clone();
    }
    titles
        .get(window.active_pane)
        .ok()
        .and_then(|title| title.0.clone())
        .unwrap_or_default()
}

/// Formats a window entry's label as `<index>:<name>`.
fn window_label(index: u32, name: &str) -> String {
    format!("{index}:{name}")
}

/// The (label, background) color pair for a window entry: an accent fill with
/// dark text when active, transparent with muted text otherwise.
fn entry_colors(is_active: bool) -> (Color, Color) {
    if is_active {
        (ACTIVE_FG, ACTIVE_BG)
    } else {
        (INACTIVE_FG, INACTIVE_BG)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(WindowBarPlugin);
        app
    }

    fn spawn_bar(app: &mut App) -> Entity {
        app.world_mut().spawn(WindowBarContainer).id()
    }

    fn texts(app: &mut App) -> Vec<String> {
        let world = app.world_mut();
        let mut texts: Vec<String> = world
            .query::<&Text>()
            .iter(world)
            .map(|t| t.0.clone())
            .collect();
        texts.sort();
        texts
    }

    #[test]
    fn window_label_formats_index_and_name() {
        assert_eq!(window_label(0, "zsh"), "0:zsh");
        assert_eq!(window_label(12, ""), "12:");
    }

    #[test]
    fn entry_colors_active_vs_inactive() {
        assert_eq!(entry_colors(true), (ACTIVE_FG, ACTIVE_BG));
        assert_eq!(entry_colors(false), (INACTIVE_FG, INACTIVE_BG));
    }

    #[test]
    fn rebuild_renders_one_entry_per_window_with_active_highlight() {
        let mut app = test_app();
        let bar = spawn_bar(&mut app);
        let pane_a = app.world_mut().spawn_empty().id();
        let pane_b = app.world_mut().spawn_empty().id();
        app.world_mut().spawn(MultiplexerWindow {
            index: 0,
            name: Some("zsh".into()),
            active_pane: pane_a,
        });
        app.world_mut().spawn((
            MultiplexerWindow {
                index: 1,
                name: Some("vim".into()),
                active_pane: pane_b,
            },
            ActiveMultiplexerWindow,
        ));

        app.update();

        let world = app.world_mut();
        let mut query = world.query::<(&WindowEntry, &BackgroundColor, &ChildOf)>();
        let mut rows: Vec<(u32, Color, Entity)> = query
            .iter(world)
            .map(|(entry, bg, child_of)| (entry.index, bg.0, child_of.parent()))
            .collect();
        rows.sort_by_key(|(index, _, _)| *index);

        assert_eq!(rows.len(), 2, "one entry per window");
        assert!(
            rows.iter().all(|(_, _, parent)| *parent == bar),
            "entries mount under the WindowBarContainer"
        );
        assert_eq!(
            rows[0].1, INACTIVE_BG,
            "inactive entry uses the inactive background"
        );
        assert_eq!(
            rows[1].1, ACTIVE_BG,
            "active entry uses the active background"
        );
        assert_eq!(
            texts(&mut app),
            vec!["0:zsh".to_string(), "1:vim".to_string()],
            "labels are <index>:<name>"
        );
    }

    #[test]
    fn unnamed_window_shows_active_pane_title() {
        let mut app = test_app();
        spawn_bar(&mut app);
        let pane = app.world_mut().spawn(TerminalTitle(None)).id();
        app.world_mut().spawn((
            MultiplexerWindow {
                index: 0,
                name: None,
                active_pane: pane,
            },
            ActiveMultiplexerWindow,
        ));

        app.update();
        assert_eq!(
            texts(&mut app),
            vec!["0:".to_string()],
            "no title yet: the unnamed window's label has an empty name"
        );

        app.world_mut().get_mut::<TerminalTitle>(pane).unwrap().0 = Some("vim".to_string());
        app.update();

        assert_eq!(
            texts(&mut app),
            vec!["0:vim".to_string()],
            "a later TerminalTitle change on the active pane must rebuild the label"
        );
    }

    #[test]
    fn click_entry_fires_select_window_request() {
        #[derive(Resource, Default)]
        struct Captured(Vec<WindowSelect>);

        fn capture(mut reader: MessageReader<SelectWindowRequest>, mut c: ResMut<Captured>) {
            for m in reader.read() {
                c.0.push(m.0);
            }
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectWindowRequest>()
            .init_resource::<Captured>()
            .add_systems(Update, (select_window_on_click, capture).chain());
        let entry = app
            .world_mut()
            .spawn((WindowEntry { index: 2 }, Interaction::None))
            .id();

        app.update();
        assert!(
            app.world().resource::<Captured>().0.is_empty(),
            "no press yet: no request fired"
        );

        *app.world_mut().get_mut::<Interaction>(entry).unwrap() = Interaction::Pressed;
        app.update();

        let captured = &app.world().resource::<Captured>().0;
        assert_eq!(captured.len(), 1, "a press fires exactly one request");
        assert!(
            matches!(captured[0], WindowSelect::Index(2)),
            "the request targets the pressed entry's index"
        );
    }

    #[test]
    fn click_entry_with_index_beyond_u8_is_not_truncated() {
        #[derive(Resource, Default)]
        struct Captured(Vec<WindowSelect>);

        fn capture(mut reader: MessageReader<SelectWindowRequest>, mut c: ResMut<Captured>) {
            for m in reader.read() {
                c.0.push(m.0);
            }
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectWindowRequest>()
            .init_resource::<Captured>()
            .add_systems(Update, (select_window_on_click, capture).chain());
        let entry = app
            .world_mut()
            .spawn((WindowEntry { index: 300 }, Interaction::None))
            .id();

        app.update();
        *app.world_mut().get_mut::<Interaction>(entry).unwrap() = Interaction::Pressed;
        app.update();

        let captured = &app.world().resource::<Captured>().0;
        assert!(
            matches!(captured[0], WindowSelect::Index(300)),
            "a window index above 255 must reach the request unwrapped"
        );
    }
}
