//! Host-side input: maintains the crate's `KeyboardDisabled` / `MouseDisabled`
//! markers from the coarse guards (IME, focus, webview). The frame's shortcut
//! messages (resolved by
//! `crate::input::keyboard::key_effect::classify_key_batch`) are applied by
//! `crate::input::shortcuts::default_mode`'s per-message systems — vi-mode
//! entry, the shared `[vi-mode]` key table (while vi mode is active),
//! direct-chord and leader paste, and raw-key typing
//! (`crate::action::clipboard::PasteAction`, `TerminalKeyInput`). Quit and
//! release-webview-focus are handled upstream; the pane/window actions are
//! no-ops.

use crate::input::InputPhase;
use crate::input::focus::{KeyboardDisabled, MouseDisabled};
use crate::input::ime::{ImeCommit, ImeState};
use crate::surface::OrzmaTerminal;
use crate::surface::geometry::phys_to_pane_local;
use crate::surface::geometry::topmost_surface_at;
use crate::ui::vi_mode::ViModeState;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, ComputedStackIndex, UiGlobalTransform};
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use orzma_tmux::TmuxPane;
use orzma_tty_engine::{TerminalKey, TerminalKeyInput, TerminalModifiers};
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_tty_renderer::prelude::TerminalOverlays;
use orzma_webview::{NonInteractive, Webview, webview_hit_at};

/// Registers the host-side input systems.
pub(super) struct DefaultHostInputPlugin;

impl Plugin for DefaultHostInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, maintain_input_gates.before(InputPhase::Hover))
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
        (
            Entity,
            &'static ComputedNode,
            &'static ComputedStackIndex,
            &'static UiGlobalTransform,
        ),
        With<OrzmaTerminal>,
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
            Has<ViModeState>,
        ),
        With<OrzmaTerminal>,
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
    for (entity, has_keyboard, has_mouse, in_vi_mode) in terminals.iter() {
        let disable_keyboard = keyboard_disable || in_vi_mode;
        let disable_mouse = mouse_modal || in_vi_mode || Some(entity) == claimed;
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
/// or `None`. Resolves the topmost `OrzmaTerminal` under the cursor, then
/// hit-tests its active overlay rects (`webview_hit_at` skips `NonInteractive`
/// children). A claimed surface is marked `MouseDisabled` so
/// `dispatch_mouse_buttons` yields the click to the webview router.
fn cursor_claims_webview(window: &Window, claim: &WebviewClaimParams) -> Option<Entity> {
    let metrics = claim.metrics.as_deref()?;
    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_phys = window.cursor_position()? * scale;
    let terminal = topmost_surface_at(cursor_phys, claim.surfaces.iter())?;
    let (_, node, _, transform) = claim.surfaces.get(terminal).ok()?;
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

fn apply_ime_commit_to_terminal(
    ev: On<ImeCommit>,
    mut commands: Commands,
    terminals: Query<(), (With<OrzmaTerminal>, Without<TmuxPane>)>,
) {
    // NOTE: discriminate on TmuxPane absence — tmux panes are also OrzmaTerminal
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
    use crate::action::clipboard::PasteAction;
    use crate::action::terminal::TerminalSelectionCopy;
    use crate::input::keyboard::key_effect::KeyEffect;
    use crate::input::shortcuts::default_mode::{
        apply_default_shortcuts, apply_default_type, apply_default_vi_mode,
    };
    use crate::input::shortcuts::{ShortcutMessage, Shortcuts, TypeMessage, ViModeMessage};
    use crate::ui::vi_mode::EnterViModeActionEvent;
    use bevy::app::App;
    use bevy::ecs::resource::Resource;
    use bevy::input::keyboard::{Key, KeyCode};
    use bevy::prelude::{Entity, MinimalPlugins, On, ResMut};
    use bevy::window::PrimaryWindow;
    use orzma_configs::shortcuts::{Modifiers, Shortcut};
    use orzma_tmux::{PaneId, TmuxPane};
    use orzma_tty_engine::TerminalKey;
    use tmux_control_parser::CellDims;

    #[test]
    fn ime_commit_fires_terminal_key_input_for_plain_terminal() {
        use crate::input::ime::ImeCommit;
        use crate::surface::OrzmaTerminal;
        use orzma_tty_engine::TerminalKey;

        #[derive(Resource, Default)]
        struct Hits(Vec<(Entity, TerminalKey)>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Hits>()
            .add_observer(apply_ime_commit_to_terminal)
            .add_observer(|ev: On<TerminalKeyInput>, mut h: ResMut<Hits>| {
                h.0.push((ev.entity, ev.key.clone()));
            });

        let term = app.world_mut().spawn(OrzmaTerminal).id();
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
        use crate::surface::OrzmaTerminal;

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
                OrzmaTerminal,
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

    /// Default shell (`OrzmaTerminal`, no `TmuxPane`) at window center (400,300),
    /// size 800x600, with one interactive inline rect rows 2..12, cols 3..43
    /// (phys y 32..192, x 24..344 at the 8x16 px cell pitch). Runs
    /// `maintain_input_gates`. Returns `(app, shell)`.
    fn make_gate_app() -> (App, Entity) {
        use bevy::math::IVec4;
        use bevy::window::WindowResolution;
        use orzma_tty_renderer::CellMetrics;

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
                OrzmaTerminal,
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
        vi_mode: u32,
        paste: u32,
        copy: u32,
        keys: Vec<TerminalKey>,
    }

    /// Builds an app running the three Default appliers as bare
    /// per-message consumers, capturing the events they trigger.
    fn build_default_dispatch_app(shortcuts: Shortcuts) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<ShortcutMessage>()
            .add_message::<ViModeMessage>()
            .add_message::<TypeMessage>()
            .init_resource::<Captured>()
            .insert_resource(shortcuts)
            .add_systems(
                Update,
                (
                    apply_default_shortcuts,
                    apply_default_vi_mode.after(apply_default_shortcuts),
                    apply_default_type
                        .after(apply_default_shortcuts)
                        .after(apply_default_vi_mode),
                ),
            )
            .add_observer(|_ev: On<EnterViModeActionEvent>, mut c: ResMut<Captured>| {
                c.vi_mode += 1;
            })
            .add_observer(|_ev: On<PasteAction>, mut c: ResMut<Captured>| {
                c.paste += 1;
            })
            .add_observer(|_ev: On<TerminalSelectionCopy>, mut c: ResMut<Captured>| {
                c.copy += 1;
            })
            .add_observer(|ev: On<TerminalKeyInput>, mut c: ResMut<Captured>| {
                c.keys.push(ev.key.clone());
            });
        app
    }

    fn default_dispatch_app(shortcuts: Shortcuts) -> (App, Entity) {
        let mut app = build_default_dispatch_app(shortcuts);
        let term = app.world_mut().spawn(OrzmaTerminal).id();
        (app, term)
    }

    fn meta_mods() -> Modifiers {
        Modifiers {
            ctrl: false,
            shift: false,
            alt: false,
            meta: true,
        }
    }

    fn dispatch(
        app: &mut App,
        effects: Vec<KeyEffect>,
        focused: Option<Entity>,
        in_vi_mode: bool,
        mods: Modifiers,
    ) {
        for effect in effects {
            match effect {
                KeyEffect::Shortcut { action, via_leader } => {
                    app.world_mut().write_message(ShortcutMessage {
                        action,
                        via_leader,
                        focused,
                        in_vi_mode,
                    });
                }
                KeyEffect::ViMode(action) => {
                    app.world_mut()
                        .write_message(ViModeMessage { action, focused });
                }
                KeyEffect::Type { logical, key_code } => {
                    app.world_mut().write_message(TypeMessage {
                        logical,
                        key_code,
                        focused,
                        mods,
                    });
                }
                KeyEffect::WebviewForward { .. } => {}
            }
        }
        app.update();
    }

    fn type_effect(logical: Key, key_code: KeyCode) -> KeyEffect {
        KeyEffect::Type { logical, key_code }
    }

    fn action_effect(action: Shortcut, via_leader: bool) -> KeyEffect {
        KeyEffect::Shortcut { action, via_leader }
    }

    #[test]
    fn plain_key_triggers_terminal_key_input() {
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![type_effect(Key::Character("a".into()), KeyCode::KeyA)],
            Some(term),
            false,
            Modifiers::default(),
        );
        assert_eq!(
            app.world().resource::<Captured>().keys,
            vec![TerminalKey::Text("a".into())],
            "a Type effect must forward to the focused terminal as a TerminalKeyInput"
        );
    }

    #[test]
    fn pane_action_is_noop() {
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::ZoomPane, true)],
            Some(term),
            false,
            Modifiers::default(),
        );
        let c = app.world().resource::<Captured>();
        assert_eq!(
            (c.vi_mode, c.paste, c.keys.len()),
            (0, 0, 0),
            "a Default-mode pane action resolves to a no-op: no event, no typing"
        );
    }

    #[test]
    fn direct_paste_outside_vi_mode_pastes() {
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Paste, false)],
            Some(term),
            false,
            meta_mods(),
        );
        assert_eq!(
            app.world().resource::<Captured>().paste,
            1,
            "a direct paste (via_leader=false) outside vi mode must fire PasteAction"
        );
    }

    #[test]
    fn direct_copy_outside_vi_mode_fires_selection_copy() {
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Copy, false)],
            Some(term),
            false,
            meta_mods(),
        );
        assert_eq!(
            app.world().resource::<Captured>().copy,
            1,
            "a direct copy must fire TerminalSelectionCopy on the focused terminal"
        );
    }

    #[test]
    fn direct_copy_in_vi_mode_also_fires_selection_copy() {
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Copy, false)],
            Some(term),
            true,
            meta_mods(),
        );
        assert_eq!(
            app.world().resource::<Captured>().copy,
            1,
            "copy fires unconditionally: vi mode must not suppress it (unlike paste)"
        );
    }

    #[test]
    fn direct_paste_in_vi_mode_suppressed() {
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Paste, false)],
            Some(term),
            true,
            meta_mods(),
        );
        assert_eq!(
            app.world().resource::<Captured>().paste,
            0,
            "a direct paste in vi mode must be suppressed (via_leader || !in_vi_mode)"
        );
    }

    #[test]
    fn leader_paste_in_vi_mode_pastes() {
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Paste, true)],
            Some(term),
            true,
            Modifiers::default(),
        );
        assert_eq!(
            app.world().resource::<Captured>().paste,
            1,
            "a leader-scoped paste (via_leader=true) must fire even in vi mode"
        );
    }

    #[test]
    fn enter_vi_mode_fires_even_when_already_in_vi_mode() {
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::EnterViMode, false)],
            Some(term),
            true,
            Modifiers::default(),
        );
        assert_eq!(
            app.world().resource::<Captured>().vi_mode,
            1,
            "EnterViMode must fire unconditionally in Default mode, even when vi mode is \
             already active"
        );
    }
}
