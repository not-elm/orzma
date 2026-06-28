//! Host-side input for `AppMode::Default`: maintains the crate's `KeyboardDisabled` / `MouseDisabled`
//! markers from the coarse guards (IME, focus, webview), and handles the
//! application-level GUI shortcuts the terminal crate does not own (Quit,
//! DetachSession, ReleaseWebviewFocus). Raw-key forwarding and paste
//! are owned by `ozma_terminal`'s dispatcher and `PasteAction`.

use crate::input::focus::MouseDisabled;
use crate::input::focus::{KeyboardDisabled, KeyboardFocused};
use crate::input::ime::{ImeCommit, ImeState};
use crate::input::shortcuts::ResolvedShortcuts;
use crate::input::{InputPhase, current_modifiers};
use crate::mode::AppMode;
use crate::surface_geom::phys_to_pane_local;
use crate::ui::copy_mode::{CopyModeState, EnterCopyModeActionEvent};
use crate::webview_pointer::topmost_surface_at;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::{OzmaTerminal, OzmaTerminalInputSet, OzmaTerminalMouseSet};
use ozma_tty_engine::{TerminalKey, TerminalKeyInput, TerminalModifiers};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozma_webview::{NonInteractive, Webview, webview_hit_at};
use ozmux_configs::shortcuts::ShortcutAction;
use ozmux_tmux::TmuxPane;

/// Registers the host-side input systems for `AppMode::Default`.
pub(crate) struct DefaultHostInputPlugin;

impl Plugin for DefaultHostInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            maintain_input_gates
                .before(OzmaTerminalInputSet)
                .before(OzmaTerminalMouseSet)
                .run_if(in_state(AppMode::Default)),
        )
        .add_systems(
            Update,
            app_shortcut_handler
                .in_set(InputPhase::FocusedKey)
                .run_if(in_state(AppMode::Default))
                .run_if(on_message::<KeyboardInput>),
        )
        .add_observer(apply_ime_commit_to_terminal);
    }
}

/// Inline-webview hit-test inputs for the mouse rect-claim, bundled to stay
/// within Bevy's system-parameter limit. `metrics` is optional so the gate still
/// runs before cell metrics exist (no claim possible yet).
#[derive(SystemParam)]
struct WebviewClaimParams<'w, 's> {
    metrics: Option<Res<'w, TerminalCellMetricsResource>>,
    surfaces: Query<
        'w,
        's,
        (Entity, &'static ComputedNode, &'static UiGlobalTransform),
        With<OzmaTerminal>,
    >,
    children: Query<'w, 's, &'static Children>,
    webviews: Query<'w, 's, (&'static Webview, Has<NonInteractive>)>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
}

fn maintain_input_gates(
    mut commands: Commands,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminals: Query<
        (
            Entity,
            Has<KeyboardDisabled>,
            Has<MouseDisabled>,
            Has<CopyModeState>,
        ),
        With<OzmaTerminal>,
    >,
    claim: WebviewClaimParams,
) {
    let window = windows.single().ok();
    let focused = window.map(|w| w.focused).unwrap_or(false);
    let keyboard_disable =
        should_disable_input(ime.is_composing(), focused, focused_webview.0.is_some());
    // NOTE: mouse is NOT disabled on webview focus alone — only over an
    // interactive inline rect (the rect-claim) — so an off-rect click still
    // reaches `dispatch_mouse_buttons` (and clears webview focus in the router).
    // Re-adding `focused_webview.0.is_some()` here would swallow that fallthrough
    // click, stranding the user on a focused webview.
    let mouse_modal = ime.is_composing() || !focused;
    let claimed = window.and_then(|w| cursor_claims_webview(w, &claim));
    for (entity, has_keyboard, has_mouse, in_copy_mode) in terminals.iter() {
        let disable_keyboard = keyboard_disable || in_copy_mode;
        let disable_mouse = mouse_modal || in_copy_mode || Some(entity) == claimed;
        if disable_keyboard && !has_keyboard {
            commands.entity(entity).insert(KeyboardDisabled);
        } else if !disable_keyboard && has_keyboard {
            commands.entity(entity).remove::<KeyboardDisabled>();
        }
        if disable_mouse && !has_mouse {
            commands.entity(entity).insert(MouseDisabled);
        } else if !disable_mouse && has_mouse {
            commands.entity(entity).remove::<MouseDisabled>();
        }
    }
}

/// The Default shell whose INTERACTIVE inline webview rect is under the cursor,
/// or `None`. Mirrors `crate::mode::tmux::gate::claimed_webview_pane` for the single
/// Default surface: resolve the topmost `OzmaTerminal` under the cursor, then
/// hit-test its active overlay rects (`webview_hit_at` skips `NonInteractive`
/// children). A claimed surface is marked `MouseDisabled` so
/// `dispatch_mouse_buttons` yields the click to the webview router.
fn cursor_claims_webview(window: &Window, claim: &WebviewClaimParams) -> Option<Entity> {
    let metrics = claim.metrics.as_deref()?;
    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_phys = window.cursor_position()? * scale;
    let terminal = topmost_surface_at(cursor_phys, claim.surfaces.iter())?;
    let (_, node, transform) = claim.surfaces.get(terminal).ok()?;
    let local_phys = phys_to_pane_local(node, transform, cursor_phys)?;
    let overlays = claim.overlay_rects.get(terminal).ok()?;
    webview_hit_at(
        &claim.children,
        &claim.webviews,
        overlays,
        terminal,
        local_phys,
        cell_w,
        cell_h,
        scale,
    )?;
    Some(terminal)
}

fn app_shortcut_handler(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut focused_webview: ResMut<FocusedWebview>,
    shortcuts: Res<ResolvedShortcuts>,
    ime: Res<ImeState>,
    bevy_keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminal: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>,
) {
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if ime.is_composing() || !focused {
        events.clear();
        return;
    }
    let mods = current_modifiers(&bevy_keys);
    let webview_focused = focused_webview.0.is_some();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if webview_focused && shortcuts.is_release_webview_focus(ev.key_code, mods) {
            focused_webview.0 = None;
            continue;
        }
        let Some(action) = shortcuts.match_gui_action(ev.key_code, mods) else {
            continue;
        };
        if gui_action_suppressed_by_webview(webview_focused, action) {
            continue;
        }
        match action {
            ShortcutAction::Quit => {
                exit.write(AppExit::Success);
            }
            ShortcutAction::EnterCopyMode => {
                if let Ok(entity) = terminal.single() {
                    commands.trigger(EnterCopyModeActionEvent { entity });
                }
            }
            ShortcutAction::DetachSession => {}
            ShortcutAction::Paste | ShortcutAction::ReleaseWebviewFocus => {}
        }
    }
}

fn apply_ime_commit_to_terminal(
    ev: On<ImeCommit>,
    mut commands: Commands,
    terminals: Query<(), (With<OzmaTerminal>, Without<TmuxPane>)>,
) {
    // NOTE: discriminate on TmuxPane absence — tmux panes are also OzmaTerminal
    // entities (src/mode/tmux/render.rs), and their commits go out via the tmux
    // observer in src/mode/tmux/forward.rs. Without this filter the commit would be
    // double-delivered.
    if terminals.get(ev.entity).is_err() {
        return;
    }
    commands.trigger(TerminalKeyInput {
        entity: ev.entity,
        key: TerminalKey::Text(ev.text.clone()),
        modifiers: TerminalModifiers::default(),
    });
}

pub(super) fn should_disable_input(
    composing: bool,
    window_focused: bool,
    webview_focused: bool,
) -> bool {
    composing || !window_focused || webview_focused
}

fn gui_action_suppressed_by_webview(webview_focused: bool, action: ShortcutAction) -> bool {
    webview_focused && action != ShortcutAction::ReleaseWebviewFocus
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::ecs::resource::Resource;
    use bevy::prelude::{Entity, MinimalPlugins, On, ResMut};
    use ozma_tty_engine::TerminalKeyInput;
    use ozmux_tmux::{PaneId, TmuxPane};
    use tmux_control_parser::CellDims;

    #[test]
    fn ime_commit_fires_terminal_key_input_for_plain_terminal() {
        use crate::input::ime::ImeCommit;
        use ozma_terminal::OzmaTerminal;
        use ozma_tty_engine::TerminalKey;

        #[derive(Resource, Default)]
        struct Hits(Vec<(Entity, TerminalKey)>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Hits>()
            .add_observer(apply_ime_commit_to_terminal)
            .add_observer(|ev: On<TerminalKeyInput>, mut h: ResMut<Hits>| {
                h.0.push((ev.entity, ev.key.clone()));
            });

        let term = app.world_mut().spawn(OzmaTerminal).id();
        app.world_mut().trigger(ImeCommit {
            entity: term,
            text: "あ".into(),
        });
        app.update();

        assert_eq!(
            app.world().resource::<Hits>().0,
            vec![(term, TerminalKey::Text("あ".into()))]
        );
    }

    #[test]
    fn ime_commit_is_noop_for_tmux_pane_target() {
        use crate::input::ime::ImeCommit;
        use ozma_terminal::OzmaTerminal;

        #[derive(Resource, Default)]
        struct Hits(u32);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Hits>()
            .add_observer(apply_ime_commit_to_terminal)
            .add_observer(|_ev: On<TerminalKeyInput>, mut h: ResMut<Hits>| h.0 += 1);

        let pane = app
            .world_mut()
            .spawn((
                OzmaTerminal,
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 0,
                        height: 0,
                        xoff: 0,
                        yoff: 0,
                    },
                },
            ))
            .id();
        app.world_mut().trigger(ImeCommit {
            entity: pane,
            text: "x".into(),
        });
        app.update();

        assert_eq!(app.world().resource::<Hits>().0, 0);
    }

    #[test]
    fn disables_input_on_any_guard() {
        assert!(!should_disable_input(false, true, false));
        assert!(should_disable_input(true, true, false));
        assert!(should_disable_input(false, false, false));
        assert!(should_disable_input(false, true, true));
    }

    #[test]
    fn webview_focus_suppresses_all_but_release() {
        assert!(gui_action_suppressed_by_webview(true, ShortcutAction::Quit));
        assert!(gui_action_suppressed_by_webview(
            true,
            ShortcutAction::DetachSession
        ));
        assert!(gui_action_suppressed_by_webview(
            true,
            ShortcutAction::EnterCopyMode
        ));
        assert!(!gui_action_suppressed_by_webview(
            true,
            ShortcutAction::ReleaseWebviewFocus
        ));
        assert!(!gui_action_suppressed_by_webview(
            false,
            ShortcutAction::Quit
        ));
    }

    /// Default shell (`OzmaTerminal`, no `TmuxPane`) at window center (400,300),
    /// size 800x600, with one interactive inline rect rows 2..12, cols 3..43
    /// (phys y 32..192, x 24..344 at the 8x16 px cell pitch). Runs
    /// `maintain_input_gates`. Returns `(app, shell)`.
    fn make_gate_app() -> (App, Entity) {
        use bevy::math::IVec4;
        use bevy::window::WindowResolution;
        use ozma_tty_renderer::CellMetrics;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ImeState>();
        app.init_resource::<FocusedWebview>();
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        });
        app.add_systems(Update, maintain_input_gates);

        let mut overlays = TerminalOverlays::default();
        overlays.rects[0] = IVec4::new(2, 3, 10, 40);
        let shell = app
            .world_mut()
            .spawn((
                OzmaTerminal,
                ComputedNode {
                    size: Vec2::new(800.0, 600.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(400.0, 300.0),
                overlays,
            ))
            .id();
        app.world_mut().spawn((
            ChildOf(shell),
            Webview {
                view_id: "w".into(),
                instance_id: None,
                slot: 0,
            },
        ));
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        (app, shell)
    }

    fn set_gate_cursor(app: &mut App, phys: Vec2) {
        use bevy::math::DVec2;
        let win = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Window>(win)
            .unwrap()
            .set_physical_cursor_position(Some(DVec2::new(phys.x as f64, phys.y as f64)));
    }

    #[test]
    fn cursor_over_webview_rect_disables_mouse() {
        let (mut app, shell) = make_gate_app();
        set_gate_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.update();
        assert!(
            app.world().entity(shell).contains::<MouseDisabled>(),
            "the cursor over an interactive webview rect must MouseDisable the shell so \
             dispatch_mouse_buttons yields the click to the webview router"
        );
    }

    #[test]
    fn focused_webview_off_rect_keeps_mouse_enabled() {
        let (mut app, shell) = make_gate_app();
        let child = app
            .world_mut()
            .query_filtered::<Entity, With<Webview>>()
            .single(app.world())
            .unwrap();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        set_gate_cursor(&mut app, Vec2::new(400.0, 400.0));
        app.update();
        assert!(
            !app.world().entity(shell).contains::<MouseDisabled>(),
            "webview focus alone must NOT MouseDisable the shell — an off-rect click must fall \
             through to the terminal (and clear webview focus in the router)"
        );
    }
}
