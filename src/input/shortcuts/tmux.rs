//! `AppMode::Tmux`'s shortcut appliers: reads `ShortcutMessage`,
//! `ViModeMessage`, `TypeMessage`, and `WebviewForwardMessage` from
//! `resolve_key_effects` and applies them as tmux action requests, vi-mode
//! keys, and forwarded pane keystrokes.

use crate::{
    action::{
        terminal::PasteAction,
        tmux::{
            DetachSessionRequest, KillPaneRequest, KillWindowRequest, NewWindowRequest,
            NextWindowRequest, PreviousWindowRequest, RenameSessionRequest, RenameWindowRequest,
            ResizePaneRequest, SelectPaneRequest, SelectWindowRequest, SplitPaneRequest,
            ZoomPaneRequest,
        },
        vi::trigger_vi_mode_action,
    },
    app_mode::{AppMode, TmuxActiveSet},
    input::{
        shortcuts::{
            ShortcutMessage, ShortcutSet, TypeMessage, ViModeMessage, WebviewForwardMessage,
        },
        tmux::forward::ForwardPaneKeysRequest,
    },
    ui::vi_mode::EnterViModeActionEvent,
};
use bevy::input::keyboard::Key;
use bevy::{ecs::system::SystemParam, prelude::*};
use orzma_configs::shortcuts::Shortcut;
use orzma_configs::shortcuts::{
    Modifiers, PaneDirection as CfgPaneDirection, SplitOrientation as CfgSplitOrientation,
};
use orzma_tmux::{
    ActiveWindow, KeyMods, PaneDirection, SplitDirection, TmuxSession, TmuxWindow,
    bevy_key_to_tmux_name,
};

pub(super) struct ShortcutsTmuxModePlugin;

impl Plugin for ShortcutsTmuxModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                apply_tmux_shortcuts
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_message::<ShortcutMessage>),
                apply_tmux_vi_mode
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_message::<ViModeMessage>)
                    .after(apply_tmux_shortcuts),
                apply_tmux_forward
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_tmux_forward_message())
                    .after(apply_tmux_shortcuts)
                    .after(apply_tmux_vi_mode),
            )
                .in_set(TmuxActiveSet),
        );
    }
}

/// Target-entity lookups for the tmux shortcut actions, bundled to stay
/// within Bevy's system-parameter limit.
#[derive(SystemParam)]
pub(in crate::input) struct ActionTargets<'w, 's> {
    active_window: Query<'w, 's, Entity, With<ActiveWindow>>,
    session: Query<'w, 's, Entity, With<TmuxSession>>,
    windows: Query<'w, 's, (Entity, &'static TmuxWindow)>,
}

/// Applies tmux keyboard shortcuts from `ShortcutMessage`: vi-mode entry,
/// paste (`PasteAction`), detach (`DetachSessionRequest`), and the pane/window
/// action requests. `Quit` / `ReleaseWebviewFocus` are handled upstream in
/// `resolve_key_effects`. Registered in `ShortcutSet::Apply`, gated on
/// `in_state(AppMode::Tmux)` + `on_message::<ShortcutMessage>`, ordered before
/// `apply_tmux_forward`.
fn apply_tmux_shortcuts(
    mut commands: Commands,
    mut shortcuts: MessageReader<ShortcutMessage>,
    targets: ActionTargets,
) {
    for msg in shortcuts.read() {
        match msg.action {
            Shortcut::EnterViMode => {
                // NOTE: re-entry guard — re-triggering while already in vi
                // mode would double-insert ViModeState and re-enter vi mode.
                if let Some(entity) = msg.focused
                    && !msg.in_vi_mode
                {
                    commands.trigger(EnterViModeActionEvent { entity });
                }
            }
            Shortcut::Paste => {
                if let Some(entity) = msg.focused {
                    commands.trigger(PasteAction { entity });
                }
            }
            Shortcut::DetachSession => {
                if let Ok(entity) = targets.session.single() {
                    commands.trigger(DetachSessionRequest { entity });
                }
            }
            action => dispatch_tmux_action(&mut commands, action, msg.focused, &targets),
        }
    }
}

/// Applies matched `[vi-mode]` keys from `ViModeMessage` on the focused
/// pane. Registered in `ShortcutSet::Apply`, gated on `in_state(AppMode::Tmux)`
/// + `on_message::<ViModeMessage>`.
fn apply_tmux_vi_mode(mut commands: Commands, mut vi_mode: MessageReader<ViModeMessage>) {
    for msg in vi_mode.read() {
        if let Some(entity) = msg.focused {
            trigger_vi_mode_action(&mut commands, entity, msg.action);
        }
    }
}

/// Forwards typed / webview-forwarded keys to the focused pane as one
/// `ForwardPaneKeysRequest`. `TypeMessage` and `WebviewForwardMessage` never
/// coexist in a frame, and every message in a frame carries the same `focused`
/// pane, so the mapped key names accumulate into a single ordered batch. Runs
/// after the shortcut/copy appliers so their triggers are queued first (parity
/// with the old single-system order). Gated on `on_tmux_forward_message`.
fn apply_tmux_forward(
    mut commands: Commands,
    mut type_keys: MessageReader<TypeMessage>,
    mut webview_forward: MessageReader<WebviewForwardMessage>,
) {
    let mut pane = None;
    let mut names = Vec::new();
    for msg in type_keys.read() {
        push_forward_name(
            &mut names,
            &mut pane,
            msg.focused,
            &msg.logical,
            msg.key_code,
            msg.mods,
        );
    }
    for msg in webview_forward.read() {
        push_forward_name(
            &mut names,
            &mut pane,
            msg.focused,
            &msg.logical,
            msg.key_code,
            msg.mods,
        );
    }
    if let Some(entity) = pane
        && !names.is_empty()
    {
        commands.trigger(ForwardPaneKeysRequest { entity, names });
    }
}

/// Appends the tmux key name for `(logical, key_code, mods)` to `names` and
/// records the focused pane, when the key maps to a name and a pane is focused.
fn push_forward_name(
    names: &mut Vec<String>,
    pane: &mut Option<Entity>,
    focused: Option<Entity>,
    logical: &Key,
    key_code: KeyCode,
    mods: Modifiers,
) {
    let Some(entity) = focused else {
        return;
    };
    let kmods = KeyMods {
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
        super_: mods.meta,
    };
    if let Some(name) = bevy_key_to_tmux_name(logical, key_code, kmods) {
        *pane = Some(entity);
        names.push(name);
    }
}

/// Triggers the tmux pane/window action request for `action` on the resolved
/// target: pane actions on `active_entity`, window actions on the active window
/// or the display-indexed window, session-scoped actions on the session. The
/// non-tmux actions (handled by the caller before this helper) are no-ops.
fn dispatch_tmux_action(
    commands: &mut Commands,
    action: Shortcut,
    active_entity: Option<Entity>,
    targets: &ActionTargets,
) {
    match action {
        Shortcut::SelectPane(direction) => {
            if let Some(entity) = active_entity {
                commands.trigger(SelectPaneRequest {
                    entity,
                    direction: tmux_pane_direction(direction),
                });
            }
        }
        Shortcut::SplitPane(orientation) => {
            if let Some(entity) = active_entity {
                commands.trigger(SplitPaneRequest {
                    entity,
                    direction: tmux_split_direction(orientation),
                });
            }
        }
        Shortcut::KillPane => {
            if let Some(entity) = active_entity {
                commands.trigger(KillPaneRequest { entity });
            }
        }
        Shortcut::ZoomPane => {
            if let Some(entity) = active_entity {
                commands.trigger(ZoomPaneRequest { entity });
            }
        }
        Shortcut::ResizePane(direction) => {
            if let Some(entity) = active_entity {
                commands.trigger(ResizePaneRequest {
                    entity,
                    direction: tmux_pane_direction(direction),
                });
            }
        }
        Shortcut::NewWindow => {
            if let Ok(entity) = targets.session.single() {
                commands.trigger(NewWindowRequest { entity });
            }
        }
        Shortcut::NextWindow => {
            if let Ok(entity) = targets.session.single() {
                commands.trigger(NextWindowRequest { entity });
            }
        }
        Shortcut::PreviousWindow => {
            if let Ok(entity) = targets.session.single() {
                commands.trigger(PreviousWindowRequest { entity });
            }
        }
        Shortcut::SelectWindow(index) => {
            if let Some(entity) = targets
                .windows
                .iter()
                .find(|(_, window)| window.index == u32::from(index))
                .map(|(entity, _)| entity)
            {
                commands.trigger(SelectWindowRequest { entity });
            }
        }
        Shortcut::KillWindow => {
            if let Ok(entity) = targets.active_window.single() {
                commands.trigger(KillWindowRequest { entity });
            }
        }
        Shortcut::RenameWindow => {
            if let Ok(entity) = targets.active_window.single() {
                commands.trigger(RenameWindowRequest { entity });
            }
        }
        Shortcut::RenameSession => {
            if let Ok(entity) = targets.session.single() {
                commands.trigger(RenameSessionRequest { entity });
            }
        }
        Shortcut::Quit
        | Shortcut::EnterViMode
        | Shortcut::Paste
        | Shortcut::DetachSession
        | Shortcut::ReleaseWebviewFocus => {}
    }
}

/// Maps the config-facing pane direction (named after the neighbor) to the
/// tmux command enum.
pub(in crate::input) fn tmux_pane_direction(direction: CfgPaneDirection) -> PaneDirection {
    match direction {
        CfgPaneDirection::Left => PaneDirection::Left,
        CfgPaneDirection::Down => PaneDirection::Down,
        CfgPaneDirection::Up => PaneDirection::Up,
        CfgPaneDirection::Right => PaneDirection::Right,
    }
}

/// Maps the config-facing split orientation (named after the DIVIDER) to the
/// tmux flag enum (named after the layout axis) — the two cross on purpose.
pub(in crate::input) fn tmux_split_direction(orientation: CfgSplitOrientation) -> SplitDirection {
    match orientation {
        CfgSplitOrientation::Vertical => SplitDirection::Horizontal,
        CfgSplitOrientation::Horizontal => SplitDirection::Vertical,
    }
}

/// Run condition for `apply_tmux_forward`: true on any frame carrying a key to
/// forward (typed or webview-forwarded). The two never coexist in a frame.
fn on_tmux_forward_message() -> impl SystemCondition<()> {
    on_message::<TypeMessage>.or(on_message::<WebviewForwardMessage>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keyboard::key_effect::KeyEffect;
    use orzma_tmux::{PaneId, TmuxPane};

    #[derive(Resource, Default)]
    struct TmuxCaptured {
        select_pane: Vec<(Entity, PaneDirection)>,
        select_window: Vec<Entity>,
        detach: Vec<Entity>,
        forward: Vec<(Entity, Vec<String>)>,
        resize_pane: Vec<(Entity, PaneDirection)>,
    }

    /// Builds an app running the three tmux appliers (`apply_tmux_shortcuts`,
    /// `apply_tmux_vi_mode`, `apply_tmux_forward`, ordered as the real
    /// plugin orders them) with one tmux pane (the active pane / the
    /// dispatched messages' `focused`), capturing the action requests they
    /// trigger.
    fn tmux_dispatch_app() -> (App, Entity) {
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<ShortcutMessage>()
            .add_message::<ViModeMessage>()
            .add_message::<TypeMessage>()
            .add_message::<WebviewForwardMessage>()
            .init_resource::<TmuxCaptured>()
            .add_systems(
                Update,
                (
                    apply_tmux_shortcuts,
                    apply_tmux_vi_mode.after(apply_tmux_shortcuts),
                    apply_tmux_forward
                        .after(apply_tmux_shortcuts)
                        .after(apply_tmux_vi_mode),
                ),
            )
            .add_observer(|ev: On<SelectPaneRequest>, mut c: ResMut<TmuxCaptured>| {
                c.select_pane.push((ev.entity, ev.direction));
            })
            .add_observer(|ev: On<ResizePaneRequest>, mut c: ResMut<TmuxCaptured>| {
                c.resize_pane.push((ev.entity, ev.direction));
            })
            .add_observer(|ev: On<SelectWindowRequest>, mut c: ResMut<TmuxCaptured>| {
                c.select_window.push(ev.entity);
            })
            .add_observer(
                |ev: On<DetachSessionRequest>, mut c: ResMut<TmuxCaptured>| {
                    c.detach.push(ev.entity);
                },
            )
            .add_observer(
                |ev: On<ForwardPaneKeysRequest>, mut c: ResMut<TmuxCaptured>| {
                    c.forward.push((ev.entity, ev.names.clone()));
                },
            );
        let pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 80,
                    height: 24,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        (app, pane)
    }

    fn dispatch(app: &mut App, effects: Vec<KeyEffect>, focused: Option<Entity>) {
        let mods = Modifiers::default();
        for effect in effects {
            match effect {
                KeyEffect::Shortcut { action, via_leader } => {
                    app.world_mut().write_message(ShortcutMessage {
                        action,
                        via_leader,
                        focused,
                        in_vi_mode: false,
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
                KeyEffect::WebviewForward { logical, key_code } => {
                    app.world_mut().write_message(WebviewForwardMessage {
                        logical,
                        key_code,
                        focused,
                        mods,
                    });
                }
            }
        }
        app.update();
    }

    #[test]
    fn select_pane_targets_active_pane() {
        let (mut app, pane) = tmux_dispatch_app();
        dispatch(
            &mut app,
            vec![KeyEffect::Shortcut {
                action: Shortcut::SelectPane(CfgPaneDirection::Left),
                via_leader: true,
            }],
            Some(pane),
        );
        assert_eq!(
            app.world().resource::<TmuxCaptured>().select_pane,
            vec![(pane, PaneDirection::Left)],
            "a SelectPane(Left) effect must trigger SelectPaneRequest on batch.focused (active pane)"
        );
    }

    #[test]
    fn resize_pane_targets_active_pane() {
        let (mut app, pane) = tmux_dispatch_app();
        dispatch(
            &mut app,
            vec![KeyEffect::Shortcut {
                action: Shortcut::ResizePane(CfgPaneDirection::Left),
                via_leader: true,
            }],
            Some(pane),
        );
        assert_eq!(
            app.world().resource::<TmuxCaptured>().resize_pane,
            vec![(pane, PaneDirection::Left)],
            "a ResizePane(Left) effect must trigger ResizePaneRequest on the active pane"
        );
    }

    #[test]
    fn plain_keys_batch_into_one_forward_request() {
        let (mut app, pane) = tmux_dispatch_app();
        dispatch(
            &mut app,
            vec![
                KeyEffect::Type {
                    logical: Key::Character("a".into()),
                    key_code: KeyCode::KeyA,
                },
                KeyEffect::Type {
                    logical: Key::Character("b".into()),
                    key_code: KeyCode::KeyB,
                },
            ],
            Some(pane),
        );
        assert_eq!(
            app.world().resource::<TmuxCaptured>().forward,
            vec![(pane, vec!["a".to_string(), "b".to_string()])],
            "Type effects in one batch must batch into a single ForwardPaneKeysRequest"
        );
    }

    #[test]
    fn detach_triggers_detach_session_request() {
        use tmux_control_parser::SessionId;

        let (mut app, pane) = tmux_dispatch_app();
        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(1),
                name: "main".into(),
            })
            .id();
        dispatch(
            &mut app,
            vec![KeyEffect::Shortcut {
                action: Shortcut::DetachSession,
                via_leader: false,
            }],
            Some(pane),
        );
        assert_eq!(
            app.world().resource::<TmuxCaptured>().detach,
            vec![session],
            "a DetachSession effect must trigger DetachSessionRequest on the session"
        );
    }

    #[test]
    fn select_window_targets_indexed_window() {
        use tmux_control_parser::WindowId;

        let (mut app, pane) = tmux_dispatch_app();
        app.world_mut().spawn(TmuxWindow {
            id: WindowId(1),
            index: 1,
            name: "one".into(),
        });
        let window_two = app
            .world_mut()
            .spawn(TmuxWindow {
                id: WindowId(2),
                index: 2,
                name: "two".into(),
            })
            .id();
        dispatch(
            &mut app,
            vec![KeyEffect::Shortcut {
                action: Shortcut::SelectWindow(2),
                via_leader: false,
            }],
            Some(pane),
        );
        assert_eq!(
            app.world().resource::<TmuxCaptured>().select_window,
            vec![window_two],
            "SelectWindow(2) must target the window whose display index is 2"
        );
    }
}
