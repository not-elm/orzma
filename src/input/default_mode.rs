//! Host-side input for `AppMode::Default`: maintains the crate's
//! `KeyboardDisabled` / `MouseDisabled` markers from the coarse guards (IME,
//! focus, webview), and applies the frame's keyboard shortcuts by resolving
//! pressed keys through the pure `crate::input::resolve::classify_key_batch`
//! decider and triggering the matching events on the focused terminal — Quit,
//! copy-mode entry, leader sequences, the shared `[copy-mode]` key table (while
//! copy mode is active), direct-chord and leader paste, and raw-key forwarding
//! (`crate::action::terminal::PasteAction`, `TerminalKeyInput`).

mod webview;

use crate::action::terminal::PasteAction;
use crate::action::vi::{ResolvedCopyModeKeys, trigger_copy_mode_action};
use crate::app_mode::AppMode;
use crate::input::focus::MouseDisabled;
use crate::input::focus::{KeyboardDisabled, KeyboardFocused};
use crate::input::ime::{ImeCommit, ImeState};
use crate::input::keyboard::{bevy_key_to_terminal_key, current_terminal_modifiers};
use crate::input::resolve::{BatchContext, KeyEffect, classify_key_batch};
use crate::input::shortcuts::{LeaderGate, LeaderPhase, Shortcuts, clear_leader_phase};
use crate::input::{InputPhase, current_modifiers};
use crate::surface::OzmaTerminal;
use crate::surface::geometry::phys_to_pane_local;
use crate::ui::copy_mode::{CopyModeState, EnterCopyModeActionEvent};
use crate::webview_pointer::topmost_surface_at;
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::time::Real;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_tty_engine::{TerminalKey, TerminalKeyInput, TerminalModifiers};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozma_webview::{NonInteractive, Webview, webview_hit_at};
use ozmux_configs::shortcuts::ShortcutAction;
use ozmux_tmux::TmuxPane;

/// Registers the host-side input systems for `AppMode::Default`.
pub(super) struct DefaultHostInputPlugin;

impl Plugin for DefaultHostInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(webview::DefaultWebviewPointerPlugin)
            .add_systems(
                Update,
                maintain_input_gates
                    .before(InputPhase::Hover)
                    .run_if(in_state(AppMode::Default)),
            )
            .add_systems(
                Update,
                apply_default_shortcuts
                    .in_set(InputPhase::FocusedKey)
                    .in_set(LeaderGate::Advance)
                    .run_if(in_state(AppMode::Default))
                    .run_if(on_message::<KeyboardInput>),
            )
            .add_observer(apply_ime_commit_to_terminal);
    }
}

/// Returns `true` when host-side keyboard input should be suppressed: IME
/// composing, window not focused, or a webview owns the keyboard.
pub(crate) fn should_disable_input(
    composing: bool,
    window_focused: bool,
    webview_focused: bool,
) -> bool {
    composing || !window_focused || webview_focused
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
/// or `None`. Mirrors `crate::input::tmux::gate::claimed_webview_pane` for the single
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

/// Applies `AppMode::Default` keyboard shortcuts: resolves the frame's pressed
/// keys through the pure `classify_key_batch` decider, then triggers the
/// matching events on the single `KeyboardFocused` terminal — Quit, copy-mode
/// entry, paste (direct fires outside copy mode, leader fires unconditionally),
/// the shared `[copy-mode]` key table, and raw-key typing. Registered in
/// `InputPhase::FocusedKey` / `LeaderGate::Advance`, gated on
/// `in_state(AppMode::Default)` + `on_message::<KeyboardInput>`.
fn apply_default_shortcuts(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut leader_phase: ResMut<LeaderPhase>,
    shortcuts: Res<Shortcuts>,
    ime: Res<ImeState>,
    bevy_keys: Res<ButtonInput<KeyCode>>,
    resolved_copy: Res<ResolvedCopyModeKeys>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminal: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>,
    copy_modes: Query<(), With<CopyModeState>>,
) {
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if ime.is_composing() || !focused {
        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    }
    let Ok(entity) = terminal.single() else {
        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    };
    let mods = current_modifiers(&bevy_keys);
    let in_copy_mode = copy_modes.get(entity).is_ok();
    let ctx = BatchContext {
        mods,
        now: time.elapsed(),
        in_copy_mode,
        webview_focused: focused_webview.0.is_some(),
        forward_chords: &[],
    };
    let effects = classify_key_batch(
        &mut leader_phase,
        &shortcuts,
        &resolved_copy,
        events.read(),
        ctx,
    );
    for effect in effects {
        match effect {
            KeyEffect::Action {
                action: ShortcutAction::Quit,
                ..
            } => {
                exit.write(AppExit::Success);
            }
            KeyEffect::Action {
                action: ShortcutAction::EnterCopyMode,
                ..
            } => commands.trigger(EnterCopyModeActionEvent { entity }),
            KeyEffect::Action {
                action: ShortcutAction::Paste,
                via_leader,
            } => {
                if via_leader || !in_copy_mode {
                    commands.trigger(PasteAction { entity });
                }
            }
            KeyEffect::Action { .. } => {}
            KeyEffect::CopyMode(action) => trigger_copy_mode_action(&mut commands, entity, action),
            KeyEffect::Type { logical, key_code } => {
                // NOTE: a chord withheld from the PTY must never be typed. The
                // release-webview-focus chord is the one direct chord the
                // decider emits as `Type` (all others resolve to `Action`), so
                // the applier drops it here rather than forward it to the
                // terminal; tmux forwards it instead.
                if !shortcuts.is_release_webview_focus(key_code, mods)
                    && let Some(key) = bevy_key_to_terminal_key(&logical)
                {
                    commands.trigger(TerminalKeyInput {
                        entity,
                        key,
                        modifiers: current_terminal_modifiers(&bevy_keys),
                    });
                }
            }
            KeyEffect::ReleaseWebviewFocus => focused_webview.0 = None,
            KeyEffect::WebviewForward { .. } => {}
        }
    }
}

fn apply_ime_commit_to_terminal(
    ev: On<ImeCommit>,
    mut commands: Commands,
    terminals: Query<(), (With<OzmaTerminal>, Without<TmuxPane>)>,
) {
    // NOTE: discriminate on TmuxPane absence — tmux panes are also OzmaTerminal
    // entities (src/render/tmux.rs), and their commits go out via the tmux
    // observer in src/input/tmux/forward.rs. Without this filter the commit would be
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::focus::KeyboardFocused;
    use crate::input::shortcuts::{
        LeaderPhase, test_shortcuts_with_direct_chord, test_shortcuts_with_repeat_prefix,
    };
    use crate::surface::OzmaTerminal;
    use bevy::app::App;
    use bevy::app::AppExit;
    use bevy::ecs::message::MessageReader;
    use bevy::ecs::resource::Resource;
    use bevy::input::ButtonInput;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::prelude::{Entity, MinimalPlugins, On, ResMut};
    use bevy::window::PrimaryWindow;
    use ozma_tty_engine::TerminalKeyInput;
    use ozmux_configs::shortcuts::Modifiers;
    use ozmux_tmux::{PaneId, TmuxPane};
    use std::time::Duration;
    use tmux_control_parser::CellDims;

    #[test]
    fn ime_commit_fires_terminal_key_input_for_plain_terminal() {
        use crate::input::ime::ImeCommit;
        use crate::surface::OzmaTerminal;
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
        use crate::surface::OzmaTerminal;

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

    #[derive(Resource, Default)]
    struct Captured {
        copy_mode: u32,
        paste: u32,
        keys: Vec<TerminalKey>,
        quit: u32,
    }

    fn capture_app_exit(mut exits: MessageReader<AppExit>, mut captured: ResMut<Captured>) {
        captured.quit += exits.read().count() as u32;
    }

    fn default_dispatch_app(shortcuts: Shortcuts) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .add_message::<AppExit>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ImeState>()
            .init_resource::<FocusedWebview>()
            .init_resource::<LeaderPhase>()
            .init_resource::<ResolvedCopyModeKeys>()
            .init_resource::<Captured>()
            .insert_resource(shortcuts)
            .add_systems(Update, apply_default_shortcuts)
            .add_systems(Update, capture_app_exit.after(apply_default_shortcuts))
            .add_observer(
                |_ev: On<EnterCopyModeActionEvent>, mut c: ResMut<Captured>| {
                    c.copy_mode += 1;
                },
            )
            .add_observer(|_ev: On<PasteAction>, mut c: ResMut<Captured>| {
                c.paste += 1;
            })
            .add_observer(|ev: On<TerminalKeyInput>, mut c: ResMut<Captured>| {
                c.keys.push(ev.key.clone());
            });
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        let term = app.world_mut().spawn((OzmaTerminal, KeyboardFocused)).id();
        (app, term)
    }

    fn repeat_prefix(action: ShortcutAction, repeat_time: Duration) -> Shortcuts {
        test_shortcuts_with_repeat_prefix(KeyCode::KeyH, action, repeat_time)
    }

    fn meta_mods() -> Modifiers {
        Modifiers {
            ctrl: false,
            shift: false,
            alt: false,
            meta: true,
        }
    }

    fn hold_meta(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
    }

    fn hold_ctrl_shift(app: &mut App) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::ShiftLeft);
    }

    fn send_key(app: &mut App, key_code: KeyCode, logical: Key) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    fn send_key_repeat(app: &mut App, key_code: KeyCode, repeat: bool) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: Key::Character("h".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat,
            window: Entity::PLACEHOLDER,
        });
    }

    #[test]
    fn os_key_repeat_refires_inside_repeat_window() {
        let (mut app, _term) = default_dispatch_app(repeat_prefix(
            ShortcutAction::EnterCopyMode,
            Duration::from_secs(60),
        ));
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Repeat {
            deadline: Duration::from_secs(60),
        };
        send_key_repeat(&mut app, KeyCode::KeyH, true);
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().copy_mode,
            1,
            "an OS auto-repeat of a repeat-marked key must re-fire inside the window (tmux parity)"
        );
        assert!(
            matches!(
                *app.world().resource::<LeaderPhase>(),
                LeaderPhase::Repeat { .. }
            ),
            "firing must keep (re-arm) the window"
        );
    }

    #[test]
    fn os_key_repeat_outside_window_stays_passthrough() {
        let (mut app, _term) = default_dispatch_app(repeat_prefix(
            ShortcutAction::EnterCopyMode,
            Duration::from_secs(60),
        ));
        send_key_repeat(&mut app, KeyCode::KeyH, true);
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().copy_mode,
            0,
            "auto-repeat with no window open must not step the leader machine"
        );
        assert_eq!(*app.world().resource::<LeaderPhase>(), LeaderPhase::Idle);
    }

    #[test]
    fn plain_key_triggers_terminal_key_input() {
        let (mut app, _term) = default_dispatch_app(Shortcuts::default());
        send_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().keys,
            vec![TerminalKey::Text("a".into())],
            "a plain key must forward to the focused terminal as a TerminalKeyInput"
        );
    }

    #[test]
    fn pane_action_is_noop() {
        let (mut app, _term) =
            default_dispatch_app(repeat_prefix(ShortcutAction::ZoomPane, Duration::ZERO));
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        send_key(&mut app, KeyCode::KeyH, Key::Character("h".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(
            (c.copy_mode, c.paste, c.keys.len(), c.quit),
            (0, 0, 0, 0),
            "a Default-mode pane action resolves to a no-op: no event, no typing"
        );
    }

    #[test]
    fn direct_paste_outside_copy_mode_pastes() {
        let (mut app, _term) = default_dispatch_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyV,
            meta_mods(),
            ShortcutAction::Paste,
        ));
        hold_meta(&mut app);
        send_key(&mut app, KeyCode::KeyV, Key::Character("v".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().paste,
            1,
            "a direct paste chord outside copy mode must fire PasteAction"
        );
    }

    #[test]
    fn direct_paste_in_copy_mode_suppressed() {
        let (mut app, term) = default_dispatch_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyV,
            meta_mods(),
            ShortcutAction::Paste,
        ));
        app.world_mut().entity_mut(term).insert(CopyModeState);
        hold_meta(&mut app);
        send_key(&mut app, KeyCode::KeyV, Key::Character("v".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().paste,
            0,
            "a direct paste chord in copy mode must be suppressed"
        );
    }

    #[test]
    fn leader_paste_in_copy_mode_pastes() {
        let (mut app, term) =
            default_dispatch_app(repeat_prefix(ShortcutAction::Paste, Duration::ZERO));
        app.world_mut().entity_mut(term).insert(CopyModeState);
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        send_key(&mut app, KeyCode::KeyH, Key::Character("h".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().paste,
            1,
            "a leader-scoped paste must fire even in copy mode"
        );
    }

    #[test]
    fn release_webview_chord_is_not_typed() {
        let (mut app, _term) = default_dispatch_app(test_shortcuts_with_direct_chord(
            KeyCode::Escape,
            Modifiers {
                ctrl: true,
                shift: true,
                alt: false,
                meta: false,
            },
            ShortcutAction::ReleaseWebviewFocus,
        ));
        hold_ctrl_shift(&mut app);
        send_key(&mut app, KeyCode::Escape, Key::Escape);
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(
            (c.copy_mode, c.paste, c.keys.len(), c.quit),
            (0, 0, 0, 0),
            "with no webview focused the release chord (Ctrl+Shift+Escape) must be dropped by \
             the applier, never typed into the PTY as Escape"
        );
    }

    #[test]
    fn enter_copy_mode_fires_even_when_already_in_copy_mode() {
        let (mut app, term) = default_dispatch_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyS,
            meta_mods(),
            ShortcutAction::EnterCopyMode,
        ));
        app.world_mut().entity_mut(term).insert(CopyModeState);
        hold_meta(&mut app);
        send_key(&mut app, KeyCode::KeyS, Key::Character("s".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().copy_mode,
            1,
            "EnterCopyMode must fire unconditionally, even when copy mode is already active"
        );
    }
}
