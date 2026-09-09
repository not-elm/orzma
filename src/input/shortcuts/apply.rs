//! The single key-effect dispatcher: reads `KeyEffectMessage` in press
//! order and, for each effect, `commands.trigger`s the matching event —
//! vi-mode entry, paste, copy, pane split/select/kill, or typed input —
//! so the mux backend receives every effect in the order the keys were
//! pressed.

use crate::input::keyboard::terminal_modifiers;
use crate::{
    action::{
        clipboard::PasteAction,
        terminal::trigger_selection_copy,
        vi::{mode::EnterViModeActionEvent, trigger_vi_mode_action},
    },
    input::{
        keyboard::{bevy_key_to_terminal_key, key_effect::KeyEffect},
        shortcuts::{KeyEffectMessage, ShortcutSet},
    },
    session::spawn::PaneSpawnRequest,
};
use bevy::prelude::*;
use bevy_orzmux::prelude::{PaneAction, RequestActiveKeyInput, RequestPaneAction};
use orzma_configs::shortcuts::{
    PaneDirection as ConfigPaneDirection, Shortcut, SplitOrientation as ConfigSplitOrientation,
};
use orzmux::prelude::{
    NewPaneAt, PaneDirection as OrzmuxPaneDirection, PaneTarget,
    SplitOrientation as OrzmuxSplitOrientation,
};

pub(super) struct ShortcutsApplyPlugin;

impl Plugin for ShortcutsApplyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            apply_key_effects
                .in_set(ShortcutSet::Apply)
                .run_if(on_message::<KeyEffectMessage>),
        );
    }
}

/// Applies the frame's key effects in press order: shortcuts, vi-mode
/// keys, and typed keys all go through `commands.trigger`, never a
/// direct backend send, so the mux receives them in this order.
/// Registered in `ShortcutSet::Apply`, gated on
/// `on_message::<KeyEffectMessage>`.
fn apply_key_effects(mut commands: Commands, mut effects: MessageReader<KeyEffectMessage>) {
    for msg in effects.read() {
        match &msg.effect {
            KeyEffect::Shortcut { action, via_leader } => {
                apply_shortcut(
                    &mut commands,
                    *action,
                    *via_leader,
                    msg.focused,
                    msg.in_vi_mode,
                );
            }
            KeyEffect::ViMode(action) => {
                if let Some(entity) = msg.focused {
                    trigger_vi_mode_action(&mut commands, entity, *action);
                }
            }
            KeyEffect::Type { logical, .. } => {
                if msg.focused.is_some()
                    && let Some(key) = bevy_key_to_terminal_key(logical)
                {
                    commands.trigger(RequestActiveKeyInput {
                        key,
                        modifiers: terminal_modifiers(msg.mods),
                    });
                }
            }
            KeyEffect::WebviewForward { .. } => {}
        }
    }
}

/// Applies one resolved `Shortcut`: vi-mode entry, paste (a direct paste
/// fires outside vi mode; a leader paste fires unconditionally), copy
/// (fires unconditionally — vi mode included; no-selection is a no-op
/// downstream), and the pane actions (select/split/kill, targeting the
/// backend's active pane). Window actions and `Quit` /
/// `ReleaseWebviewFocus` (handled upstream in `resolve_key_effects`) are
/// no-ops until the built-in multiplexer grows window support.
fn apply_shortcut(
    commands: &mut Commands,
    action: Shortcut,
    via_leader: bool,
    focused: Option<Entity>,
    in_vi_mode: bool,
) {
    match action {
        Shortcut::EnterViMode => {
            if let Some(entity) = focused {
                commands.trigger(EnterViModeActionEvent { entity });
            }
        }
        Shortcut::Paste => {
            if let Some(entity) = focused
                && (via_leader || !in_vi_mode)
            {
                commands.trigger(PasteAction { entity });
            }
        }
        Shortcut::Copy => trigger_selection_copy(commands, focused),
        Shortcut::SelectPane(direction) => commands.trigger(RequestPaneAction {
            action: PaneAction::SelectDirection(pane_direction(direction)),
        }),
        Shortcut::SplitPane(orientation) => commands.trigger(PaneSpawnRequest {
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: split_orientation(orientation),
            },
        }),
        Shortcut::KillPane => commands.trigger(RequestPaneAction {
            action: PaneAction::Kill,
        }),
        Shortcut::ResizePane(_)
        | Shortcut::ZoomPane
        | Shortcut::NewWindow
        | Shortcut::KillWindow
        | Shortcut::NextWindow
        | Shortcut::PreviousWindow
        | Shortcut::SelectWindow(_)
        | Shortcut::RenameWindow
        | Shortcut::Quit
        | Shortcut::ReleaseWebviewFocus => {}
    }
}

/// Converts `orzma_configs`' shortcut-facing pane direction to the mux
/// backend's. The two crates must not depend on each other, so orphan
/// rules forbid a `From` impl here; this match is the conversion.
fn pane_direction(direction: ConfigPaneDirection) -> OrzmuxPaneDirection {
    match direction {
        ConfigPaneDirection::Left => OrzmuxPaneDirection::Left,
        ConfigPaneDirection::Down => OrzmuxPaneDirection::Down,
        ConfigPaneDirection::Up => OrzmuxPaneDirection::Up,
        ConfigPaneDirection::Right => OrzmuxPaneDirection::Right,
    }
}

/// Converts `orzma_configs`' shortcut-facing split orientation to the mux
/// backend's, for the same orphan-rule reason as `pane_direction`.
fn split_orientation(orientation: ConfigSplitOrientation) -> OrzmuxSplitOrientation {
    match orientation {
        ConfigSplitOrientation::Vertical => OrzmuxSplitOrientation::Vertical,
        ConfigSplitOrientation::Horizontal => OrzmuxSplitOrientation::Horizontal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::terminal::TerminalSelectionCopy;
    use crate::input::shortcuts::Shortcuts;
    use crate::surface::OrzmaTerminal;
    use bevy::ecs::resource::Resource;
    use bevy::input::keyboard::{Key, KeyCode};
    use bevy::prelude::{Entity, MinimalPlugins, On, ResMut};
    use orzma_configs::shortcuts::{Modifiers, PaneDirection, SplitOrientation};
    use orzma_tty::prelude::TerminalKey;
    use orzmux::prelude::PaneDirection as OrzmuxDirection;

    #[derive(Resource, Default)]
    struct Captured {
        order: Vec<String>,
        spawns: Vec<NewPaneAt>,
        pane_actions: Vec<PaneAction>,
        paste: u32,
        copy: u32,
        vi_mode: u32,
    }

    /// Builds an app running the dispatcher as a bare per-message
    /// consumer, capturing every event it triggers in trigger order.
    fn build_dispatch_app(shortcuts: Shortcuts) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyEffectMessage>()
            .init_resource::<Captured>()
            .insert_resource(shortcuts)
            .add_systems(Update, apply_key_effects)
            .add_observer(|_ev: On<EnterViModeActionEvent>, mut c: ResMut<Captured>| {
                c.vi_mode += 1;
                c.order.push("vi".into());
            })
            .add_observer(|_ev: On<PasteAction>, mut c: ResMut<Captured>| {
                c.paste += 1;
                c.order.push("paste".into());
            })
            .add_observer(|_ev: On<TerminalSelectionCopy>, mut c: ResMut<Captured>| {
                c.copy += 1;
                c.order.push("copy".into());
            })
            .add_observer(|ev: On<RequestActiveKeyInput>, mut c: ResMut<Captured>| {
                let TerminalKey::Character(text) = &ev.key else {
                    return;
                };
                c.order.push(format!("key:{}", text.as_str()));
            })
            .add_observer(|ev: On<RequestPaneAction>, mut c: ResMut<Captured>| {
                c.pane_actions.push(ev.action);
                if let PaneAction::SelectDirection(d) = ev.action {
                    c.order.push(format!("pane:{d:?}"));
                }
            })
            .add_observer(|ev: On<PaneSpawnRequest>, mut c: ResMut<Captured>| c.spawns.push(ev.at));
        app
    }

    fn dispatch_app(shortcuts: Shortcuts) -> (App, Entity) {
        let mut app = build_dispatch_app(shortcuts);
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
            app.world_mut().write_message(KeyEffectMessage {
                effect,
                focused,
                in_vi_mode,
                mods,
            });
        }
    }

    fn type_effect(logical: Key, key_code: KeyCode) -> KeyEffect {
        KeyEffect::Type { logical, key_code }
    }

    fn action_effect(action: Shortcut, via_leader: bool) -> KeyEffect {
        KeyEffect::Shortcut { action, via_leader }
    }

    /// Asserts a `Type` key effect on a focused terminal fires
    /// `RequestActiveKeyInput` carrying the typed character.
    ///
    /// Case: the user types the plain character `a` with no shortcut chord
    /// matched, and the focused terminal must receive it as typed text.
    #[test]
    fn plain_key_triggers_request_active_key_input() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![type_effect(Key::Character("a".into()), KeyCode::KeyA)],
            Some(term),
            false,
            Modifiers::default(),
        );
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().order,
            vec!["key:a"],
            "a Type effect must forward to the active pane as a RequestActiveKeyInput"
        );
    }

    /// Asserts that same-frame effects are applied in press order, so a
    /// pane switch between two typed keys lands between them.
    ///
    /// Case: the user types `x`, presses select-right-pane, and types `y`
    /// within one frame.
    #[test]
    fn effects_apply_in_press_order() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![
                KeyEffect::Type {
                    logical: Key::Character("x".into()),
                    key_code: KeyCode::KeyX,
                },
                KeyEffect::Shortcut {
                    action: Shortcut::SelectPane(PaneDirection::Right),
                    via_leader: true,
                },
                KeyEffect::Type {
                    logical: Key::Character("y".into()),
                    key_code: KeyCode::KeyY,
                },
            ],
            Some(term),
            false,
            Modifiers::default(),
        );
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().order,
            vec!["key:x", "pane:Right", "key:y"],
            "a same-frame pane switch must land between the two typed keys, not after both"
        );
    }

    /// Asserts that split / kill map to their pane requests with an
    /// `Active` target.
    ///
    /// Case: the user presses split-vertical-pane then kill-pane.
    #[test]
    fn split_and_kill_map_to_pane_requests() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![
                KeyEffect::Shortcut {
                    action: Shortcut::SplitPane(SplitOrientation::Vertical),
                    via_leader: true,
                },
                KeyEffect::Shortcut {
                    action: Shortcut::KillPane,
                    via_leader: true,
                },
            ],
            Some(term),
            false,
            Modifiers::default(),
        );
        app.update();
        let c = app.world().resource::<Captured>();
        assert!(matches!(
            c.spawns.as_slice(),
            [NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: OrzmuxSplitOrientation::Vertical,
            }]
        ));
        assert_eq!(c.pane_actions, vec![PaneAction::Kill]);
    }

    /// Asserts that `SelectPane` maps to `RequestPaneAction::SelectDirection`
    /// carrying the direction converted to the mux backend's type.
    ///
    /// Case: the user presses a leader-scoped select-left-pane binding.
    #[test]
    fn select_pane_maps_to_request_pane_action() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(
                Shortcut::SelectPane(PaneDirection::Left),
                true,
            )],
            Some(term),
            false,
            Modifiers::default(),
        );
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().pane_actions,
            vec![PaneAction::SelectDirection(OrzmuxDirection::Left)],
            "SelectPane must map to RequestPaneAction::SelectDirection with the converted direction"
        );
    }

    /// Asserts that a pane action with no backend mapping yet resolves to a
    /// no-op: no triggered event, spawn request, or pane action.
    ///
    /// Case: the user presses a leader-scoped zoom-pane binding.
    #[test]
    fn pane_action_without_backend_variant_is_noop() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::ZoomPane, true)],
            Some(term),
            false,
            Modifiers::default(),
        );
        app.update();
        let c = app.world().resource::<Captured>();
        assert!(
            c.order.is_empty() && c.spawns.is_empty() && c.pane_actions.is_empty(),
            "a pane action with no backend mapping yet must resolve to a no-op"
        );
    }

    #[test]
    fn direct_paste_outside_vi_mode_pastes() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Paste, false)],
            Some(term),
            false,
            meta_mods(),
        );
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().paste,
            1,
            "a direct paste (via_leader=false) outside vi mode must fire PasteAction"
        );
    }

    #[test]
    fn direct_copy_outside_vi_mode_fires_selection_copy() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Copy, false)],
            Some(term),
            false,
            meta_mods(),
        );
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().copy,
            1,
            "a direct copy must fire TerminalSelectionCopy on the focused terminal"
        );
    }

    #[test]
    fn direct_copy_in_vi_mode_also_fires_selection_copy() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Copy, false)],
            Some(term),
            true,
            meta_mods(),
        );
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().copy,
            1,
            "copy fires unconditionally: vi mode must not suppress it (unlike paste)"
        );
    }

    #[test]
    fn direct_paste_in_vi_mode_suppressed() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Paste, false)],
            Some(term),
            true,
            meta_mods(),
        );
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().paste,
            0,
            "a direct paste in vi mode must be suppressed (via_leader || !in_vi_mode)"
        );
    }

    #[test]
    fn leader_paste_in_vi_mode_pastes() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Paste, true)],
            Some(term),
            true,
            Modifiers::default(),
        );
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().paste,
            1,
            "a leader-scoped paste (via_leader=true) must fire even in vi mode"
        );
    }

    #[test]
    fn enter_vi_mode_fires_even_when_already_in_vi_mode() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::EnterViMode, false)],
            Some(term),
            true,
            Modifiers::default(),
        );
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().vi_mode,
            1,
            "EnterViMode must fire unconditionally, even when vi mode is already active"
        );
    }
}
