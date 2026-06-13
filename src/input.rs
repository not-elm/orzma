//! Keyboard shortcut handling: dispatcher systems. The shortcut binding
//! table comes from the loaded `OzmuxConfigsResource`; this module owns
//! no chord data.

pub(crate) mod hyperlink;
pub(crate) mod ime;
pub(crate) mod mouse_buttons;
pub(crate) mod mouse_wheel;

use crate::action::close_pane::ClosePaneActionEvent;
use crate::action::close_surface::CloseSurfaceActionEvent;
use crate::action::focus_pane::FocusPaneActionEvent;
use crate::action::focus_surface::FocusSurfaceActionEvent;
use crate::action::new_terminal_surface::NewTerminalSurfaceActionEvent;
use crate::action::split_pane::SplitPaneActionEvent;
use crate::action::swap_pane::SwapPaneActionEvent;
use crate::action::workspace::{
    FocusWorkspaceActionEvent, FocusWorkspaceTarget, NewWorkspaceActionEvent,
};
use crate::clipboard::{Clipboard, CopyToClipboardActionEvent, PasteFromClipboardActionEvent};
use crate::configs::OzmuxConfigsResource;
use crate::inline_webview::{InlineWebview, focused_inline_of};
use crate::input::ime::{ImeState, read_ime_events};
use crate::system_set::OzmuxSystems;
use crate::ui::copy_mode::{
    CopyModeState, EnterCopyModeActionEvent, dispatch_key as dispatch_copy_mode_key,
};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy_cef::prelude::FocusedWebview;
use bevy_terminal::{TerminalKey, TerminalKeyInput, TerminalModifiers};
use ozmux_configs::shortcuts::{
    Direction as ConfigDirection, KeyChord, Modifiers, ShortcutAction, SplitDirection,
    SurfaceOffset as ConfigSurfaceOffset, SwapOffset as ConfigSwapOffset, WorkspaceOffset,
};
use ozmux_multiplexer::{
    ActivePane, ActiveSurface, AttachedWorkspace, CycleDirection, MultiplexerCommands,
    PaneDirection, SplitOrientation, SwapOffset, WorkspaceMarker,
};
use std::collections::HashSet;

/// Resolves the focused surface's entity via the attached workspace →
/// active pane → active surface chain. The Surface entity *is* its own host,
/// so the active surface entity is returned directly.
pub(crate) fn resolve_focused_terminal(
    mux: &MultiplexerCommands,
    attached_workspace: &Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
) -> Option<Entity> {
    let workspace = attached_workspace.iter().next()?;
    resolve_active_surface_entity(mux, workspace)
}

/// Resolves the focused surface's entity using plain read-only `ActivePane` /
/// `ActiveSurface` queries instead of the full `MultiplexerCommands` SystemParam.
/// Systems that mutate `Node`/`Children` cannot also hold `MultiplexerCommands`
/// (its broad `&Node`/`&Children` layout queries alias the mutation — Bevy
/// B0001), so they resolve the focused terminal through this narrow path.
pub(crate) fn resolve_focused_terminal_readonly(
    attached_workspace: &Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    active_panes: &Query<&ActivePane>,
    active_surfaces: &Query<&ActiveSurface>,
) -> Option<Entity> {
    let workspace = attached_workspace.iter().next()?;
    let pane = active_panes.get(workspace).ok()?.0;
    active_surfaces.get(pane).ok().map(|s| s.0)
}

/// Sub-phases of `OzmuxSystems::Input`. Runs in the order:
/// `Hover` (cursor / hyperlink hover detection) → `Dispatch`
/// (mouse / wheel button routing) → `FocusedKey` (keyboard
/// shortcut + key forwarding).
#[derive(SystemSet, Hash, PartialEq, Eq, Debug, Clone)]
pub(crate) enum InputPhase {
    Hover,
    Dispatch,
    FocusedKey,
}

/// Bevy Plugin that registers the keyboard shortcut handling pipeline.
pub struct OzmuxShortcutPlugin;

impl Plugin for OzmuxShortcutPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            Update,
            (
                InputPhase::Hover,
                InputPhase::Dispatch,
                InputPhase::FocusedKey,
            )
                .chain()
                .in_set(OzmuxSystems::Input),
        )
        .add_systems(
            Update,
            dispatch_focused_key
                .run_if(not(is_ime_composing))
                .in_set(InputPhase::FocusedKey)
                .after(read_ime_events),
        );
    }
}

pub(crate) fn dispatch_focused_key(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    mut clipboard: ResMut<Clipboard>,
    mut handles: Query<(
        &mut bevy_terminal::TerminalHandle,
        &mut bevy_terminal::PtyHandle,
        &mut bevy_terminal::Coalescer,
    )>,
    mut focused_webview: Option<ResMut<FocusedWebview>>,
    mux: MultiplexerCommands,
    windows: Query<&Window>,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<OzmuxConfigsResource>,
    copy_modes: Query<(), With<CopyModeState>>,
    inline_parents: Query<&ChildOf, With<InlineWebview>>,
) {
    let bindings = &configs.shortcuts.bindings;
    // NOTE: ButtonInput<KeyCode> is updated in PreUpdate; every Update-tick event
    // sees the same modifier snapshot. Read once outside the loop.
    let mods = current_modifiers(&keys);
    let mods_empty = mods == Modifiers::default();
    let mut just_exited: HashSet<Entity> = HashSet::new();

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }

        let Ok(win) = windows.get(ev.window) else {
            continue;
        };
        if !win.focused {
            continue;
        }

        let workspace = match attached_workspace.single() {
            Ok(e) => e,
            Err(err) => {
                // NOTE: silently dropping keystrokes here would be invisible to
                // the user. The invariant 'exactly one entity carries
                // AttachedWorkspace' is enforced by bootstrap and the observers in
                // `crate::action::workspace`; if it's violated we want a loud
                // signal in the log so the failure mode is observable.
                tracing::warn!(
                    target: "ozmux_gui::input",
                    ?err,
                    "attached_workspace.single() failed; dropping keystroke (AttachedWorkspace invariant violated)"
                );
                continue;
            }
        };

        let active_pane = mux.workspaces_active_pane(workspace);
        let focused_entity = active_pane.and_then(|p| mux.panes_active_surface(p));
        let focused_inline =
            focused_inline_of(focused_webview.as_deref(), &inline_parents, focused_entity);

        // NOTE: the release-chord check is hoisted ABOVE both the Escape
        // pre-handler and the copy-mode gate (spec §7): the pre-handler would
        // otherwise consume the chord's Escape while scrolled back (snapping
        // the viewport and culling the focused webview), and copy mode would
        // swallow it outright — leaving no way to release inline focus.
        if focused_inline.is_some()
            && let Some(release_chord) = bindings.release_inline_focus.as_ref()
            && bevy_to_configs_key(&ev.logical_key)
                .is_some_and(|key| release_chord.key == key && release_chord.modifiers == mods)
        {
            if ev.repeat {
                continue;
            }
            if let Some(ref mut fw) = focused_webview {
                fw.0 = None;
            }
            continue;
        }

        if matches!(ev.logical_key, Key::Escape)
            && mods_empty
            && focused_inline.is_none()
            && let Some(entity) = focused_entity
            && copy_modes.get(entity).is_err()
            && let Ok((mut handle, _pty, mut coalescer)) = handles.get_mut(entity)
            && !handle.is_at_bottom()
        {
            handle.scroll_to_bottom(&mut coalescer);
            continue;
        }

        // NOTE: a focused inline webview wins over copy mode (same precedence
        // as the wheel path in `mouse_wheel.rs`): without the `focused_inline`
        // guard the copy-mode gate would consume keystrokes before the
        // inline-focus PTY suppression below, so keys would drive the vi cursor
        // instead of reaching the focused page while copy mode is active.
        if let Some(entity) = focused_entity
            && focused_inline.is_none()
            && copy_modes.get(entity).is_ok()
            && !just_exited.contains(&entity)
        {
            let exited = dispatch_copy_mode_key(
                &mut commands,
                &mut handles,
                &mut clipboard,
                entity,
                ev.logical_key.clone(),
                mods.clone(),
            );
            if exited {
                just_exited.insert(entity);
            }
            continue;
        }

        if is_modifier_only_key(&ev.logical_key) {
            continue;
        }

        if let Some(input_key) = bevy_to_configs_key(&ev.logical_key) {
            let chord = KeyChord {
                key: input_key,
                modifiers: mods.clone(),
            };
            if let Some(action) = bindings.lookup(&chord) {
                // OS key-repeat suppression: only block shortcut actions, not terminal input.
                if ev.repeat {
                    continue;
                }
                execute_action(
                    focused_webview.as_deref_mut(),
                    &mut commands,
                    &mux,
                    action,
                    workspace,
                );
                continue;
            }
        }

        if focused_inline.is_some() {
            continue;
        }

        if let Some(tk) = bevy_to_terminal_key(&ev.logical_key) {
            forward_to_active_terminal(
                &mut commands,
                &mux,
                workspace,
                tk,
                shortcut_mods_to_terminal_mods(&mods),
            );
        }
    }
}

pub(crate) fn current_modifiers(keys: &ButtonInput<KeyCode>) -> Modifiers {
    Modifiers {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        meta: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}

fn is_modifier_only_key(key: &Key) -> bool {
    matches!(
        key,
        Key::Shift
            | Key::Control
            | Key::Alt
            | Key::Super
            | Key::Meta
            | Key::Hyper
            | Key::AltGraph
            | Key::Fn
            | Key::Symbol
    )
}

fn bevy_to_configs_key(key: &Key) -> Option<ozmux_configs::shortcuts::Key> {
    use ozmux_configs::shortcuts::Key as CKey;
    Some(match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            if c.is_ascii_alphabetic() {
                return Some(CKey::Char(c.to_ascii_lowercase()));
            }
            // NOTE: Symbol+Shift reverse-normalize on US ASCII layout. macOS
            // reports the shifted glyph (e.g., '{' when Shift+'[' is pressed),
            // but our bindings target the unshifted glyph and carry Shift in
            // modifiers. Without this map, `Cmd+Shift+[` defaults never match.
            match c {
                '{' => CKey::Char('['),
                '}' => CKey::Char(']'),
                '<' => CKey::Char(','),
                '>' => CKey::Char('.'),
                '?' => CKey::Char('/'),
                ':' => CKey::Char(';'),
                '"' => CKey::Char('\''),
                '|' => CKey::Char('\\'),
                '~' => CKey::Char('`'),
                '_' => CKey::Char('-'),
                '+' => CKey::Plus,
                '!' => CKey::Char('1'),
                '@' => CKey::Char('2'),
                '#' => CKey::Char('3'),
                '$' => CKey::Char('4'),
                '%' => CKey::Char('5'),
                '^' => CKey::Char('6'),
                '&' => CKey::Char('7'),
                '*' => CKey::Char('8'),
                '(' => CKey::Char('9'),
                ')' => CKey::Char('0'),
                _ => CKey::Char(c),
            }
        }
        Key::Escape => CKey::Escape,
        Key::Enter => CKey::Enter,
        Key::Tab => CKey::Tab,
        Key::Backspace => CKey::Backspace,
        Key::Space => CKey::Space,
        Key::ArrowUp => CKey::ArrowUp,
        Key::ArrowDown => CKey::ArrowDown,
        Key::ArrowLeft => CKey::ArrowLeft,
        Key::ArrowRight => CKey::ArrowRight,
        _ => return None,
    })
}

/// Executes a resolved `ShortcutAction` for the given workspace entity by
/// triggering the matching action `EntityEvent`.
///
/// This is the single shortcut-dispatch point: copy-mode / clipboard
/// actions target the active Terminal Surface; workspace and pane/surface
/// actions target `workspace`; not-yet-implemented variants fall through to a
/// `tracing::debug!` log.
///
/// `focused_webview` is `Option<>` so callers that run without `bevy_cef`
/// (e.g., unit tests) can pass `None` and remain green.
fn execute_action(
    mut focused_webview: Option<&mut FocusedWebview>,
    commands: &mut Commands,
    mux: &MultiplexerCommands,
    action: ShortcutAction,
    workspace: Entity,
) {
    match &action {
        ShortcutAction::EnterCopyMode => {
            if let Some(entity) = resolve_active_surface_entity(mux, workspace) {
                commands.trigger(EnterCopyModeActionEvent { entity });
            }
        }
        ShortcutAction::NewWorkspace => {
            commands.trigger(NewWorkspaceActionEvent { workspace });
        }
        ShortcutAction::FocusWorkspace { offset } => {
            commands.trigger(FocusWorkspaceActionEvent {
                workspace,
                target: match offset {
                    WorkspaceOffset::Next => FocusWorkspaceTarget::Next,
                    WorkspaceOffset::Prev => FocusWorkspaceTarget::Prev,
                    WorkspaceOffset::Last => FocusWorkspaceTarget::Last,
                },
            });
        }
        ShortcutAction::FocusWorkspaceNumber { index } => {
            commands.trigger(FocusWorkspaceActionEvent {
                workspace,
                target: FocusWorkspaceTarget::Number(*index),
            });
        }
        ShortcutAction::Copy => {
            if let Some(entity) = resolve_active_surface_entity(mux, workspace) {
                commands.trigger(CopyToClipboardActionEvent { entity });
            }
        }
        ShortcutAction::Paste => {
            if let Some(entity) = resolve_active_surface_entity(mux, workspace) {
                commands.trigger(PasteFromClipboardActionEvent { entity });
            }
        }
        ShortcutAction::SplitPane { direction } => {
            commands.trigger(SplitPaneActionEvent {
                workspace,
                orientation: split_orientation(direction.clone()),
            });
        }
        ShortcutAction::NewTerminalSurface => {
            commands.trigger(NewTerminalSurfaceActionEvent { workspace });
        }
        ShortcutAction::FocusPane { direction } => {
            commands.trigger(FocusPaneActionEvent {
                workspace,
                direction: focus_direction(direction.clone()),
            });
        }
        ShortcutAction::FocusSurface { offset } => {
            if let Some(direction) = cycle_direction(offset.clone()) {
                commands.trigger(FocusSurfaceActionEvent {
                    workspace,
                    direction,
                });
            }
        }
        ShortcutAction::SwapPane { offset } => {
            commands.trigger(SwapPaneActionEvent {
                workspace,
                offset: swap_offset(offset.clone()),
            });
        }
        ShortcutAction::ClosePane => {
            commands.trigger(ClosePaneActionEvent { workspace });
        }
        ShortcutAction::CloseSurface => {
            commands.trigger(CloseSurfaceActionEvent { workspace });
        }
        ShortcutAction::ReleaseInlineFocus => {
            if let Some(ref mut fw) = focused_webview {
                fw.0 = None;
            }
        }
        other => tracing::debug!(
            target: "ozmux_gui::input",
            ?other,
            "shortcut action not yet implemented"
        ),
    }
}

/// Resolves the active surface's entity for `workspace` via the
/// workspace → pane → surface chain. The Surface entity *is* its own host,
/// so the active surface entity is returned directly. Returns `None` when the
/// workspace has no active pane/surface.
fn resolve_active_surface_entity(mux: &MultiplexerCommands, workspace: Entity) -> Option<Entity> {
    let pane = mux.workspaces_active_pane(workspace)?;
    mux.panes_active_surface(pane)
}

fn split_orientation(d: SplitDirection) -> SplitOrientation {
    match d {
        SplitDirection::Horizontal => SplitOrientation::Horizontal,
        SplitDirection::Vertical => SplitOrientation::Vertical,
    }
}

fn focus_direction(d: ConfigDirection) -> PaneDirection {
    match d {
        ConfigDirection::Up => PaneDirection::Up,
        ConfigDirection::Down => PaneDirection::Down,
        ConfigDirection::Left => PaneDirection::Left,
        ConfigDirection::Right => PaneDirection::Right,
    }
}

fn swap_offset(o: ConfigSwapOffset) -> SwapOffset {
    match o {
        ConfigSwapOffset::Prev => SwapOffset::Prev,
        ConfigSwapOffset::Next => SwapOffset::Next,
    }
}

fn cycle_direction(o: ConfigSurfaceOffset) -> Option<CycleDirection> {
    match o {
        ConfigSurfaceOffset::Next => Some(CycleDirection::Next),
        ConfigSurfaceOffset::Prev => Some(CycleDirection::Prev),
        ConfigSurfaceOffset::Last => None,
    }
}

/// Translates a Bevy logical key into the `TerminalKey` variant the
/// `bevy_terminal` codec accepts. Returns `None` for keys the terminal
/// does not consume (F-keys, modifier-only keys, etc. — those keys are
/// silently dropped).
fn bevy_to_terminal_key(key: &Key) -> Option<TerminalKey> {
    Some(match key {
        Key::Character(s) => TerminalKey::Text(s.to_string()),
        Key::Space => TerminalKey::Text(" ".into()),
        Key::Enter => TerminalKey::Enter,
        Key::Backspace => TerminalKey::Backspace,
        Key::Tab => TerminalKey::Tab,
        Key::Escape => TerminalKey::Escape,
        Key::Delete => TerminalKey::Delete,
        Key::ArrowUp => TerminalKey::ArrowUp,
        Key::ArrowDown => TerminalKey::ArrowDown,
        Key::ArrowLeft => TerminalKey::ArrowLeft,
        Key::ArrowRight => TerminalKey::ArrowRight,
        Key::Home => TerminalKey::Home,
        Key::End => TerminalKey::End,
        Key::PageUp => TerminalKey::PageUp,
        Key::PageDown => TerminalKey::PageDown,
        _ => return None,
    })
}

/// Converts shortcut-layer `Modifiers` into the `TerminalModifiers` carried
/// on the `TerminalKeyInput` EntityEvent. MVP only reads `ctrl` on the
/// receiving side; the other fields are forwarded for future use.
fn shortcut_mods_to_terminal_mods(m: &Modifiers) -> TerminalModifiers {
    TerminalModifiers {
        ctrl: m.ctrl,
        shift: m.shift,
        alt: m.alt,
        meta: m.meta,
    }
}

/// Resolves the active surface entity for `workspace` and triggers a
/// `TerminalKeyInput` on it. Silently no-ops when the workspace has no
/// active pane/surface yet, or when the target entity has no
/// `TerminalHandle` (e.g. an extension surface) — the `bevy_terminal`
/// observer handles that case by also no-op'ing.
fn forward_to_active_terminal(
    commands: &mut Commands,
    mux: &MultiplexerCommands,
    workspace: Entity,
    key: TerminalKey,
    mods: TerminalModifiers,
) {
    let Some(pane) = mux.workspaces_active_pane(workspace) else {
        return;
    };
    let Some(entity) = mux.panes_active_surface(pane) else {
        return;
    };
    commands.trigger(TerminalKeyInput {
        entity,
        key,
        modifiers: mods,
    });
}

fn is_ime_composing(ime_state: Res<ImeState>) -> bool {
    ime_state.is_composing()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configs::OzmuxConfigsResource;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key as Bk, KeyboardInput, NativeKeyCode};
    use bevy::window::{Window, WindowResolution};
    use ozmux_configs::OzmuxConfigs;
    use ozmux_configs::shortcuts::Key as CKey;
    use ozmux_multiplexer::{AttachedWorkspace, MultiplexerPlugin, WorkspaceMarker};

    fn make_app(window_focused: bool) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(crate::action::workspace::OzmuxWorkspaceActionPlugin)
            .add_systems(Update, dispatch_focused_key.run_if(not(is_ime_composing)));
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(OzmuxConfigsResource(OzmuxConfigs::default()));
        app.init_resource::<crate::input::ime::ImeState>();
        app.insert_resource(crate::clipboard::Clipboard::new());
        app.add_plugins(crate::clipboard::ClipboardActionPlugin);
        app.add_message::<KeyboardInput>();

        let workspace = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("default".into()))
            })
            .unwrap()
            .workspace;
        app.world_mut().flush();
        // Mark the workspace entity with AttachedWorkspace (mirrors bootstrap).
        app.world_mut()
            .entity_mut(workspace)
            .insert(AttachedWorkspace);

        let window_entity = app
            .world_mut()
            .spawn(Window {
                focused: window_focused,
                resolution: WindowResolution::new(800, 600),
                ..default()
            })
            .id();
        (app, window_entity)
    }

    fn press(app: &mut App, window: Entity, key: Bk) {
        let ev = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified),
            logical_key: key,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        };
        let mut events = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>();
        events.write(ev);
    }

    use bevy::ecs::observer::On;
    use bevy_terminal::TerminalKeyInput;
    use std::sync::{Arc, Mutex};

    #[derive(Resource, Default, Clone)]
    struct CapturedKeys(Arc<Mutex<Vec<TerminalKeyInput>>>);

    fn capture_key_input(ev: On<TerminalKeyInput>, captured: Res<CapturedKeys>) {
        captured.0.lock().unwrap().push((*ev).clone());
    }

    #[derive(Resource, Default, Clone)]
    struct CapturedClipboardOps(Arc<Mutex<Vec<&'static str>>>);

    fn capture_copy_op(
        _ev: On<crate::clipboard::CopyToClipboardActionEvent>,
        cap: Res<CapturedClipboardOps>,
    ) {
        cap.0.lock().unwrap().push("copy");
    }

    fn capture_paste_op(
        _ev: On<crate::clipboard::PasteFromClipboardActionEvent>,
        cap: Res<CapturedClipboardOps>,
    ) {
        cap.0.lock().unwrap().push("paste");
    }

    /// Resolves the active pane's Surface entity in the only window of the test
    /// app, returning its Entity id. The surface carries NO `TerminalHandle`,
    /// so the `bevy_terminal` observer no-ops on the missing component — the
    /// test capture observer still records the trigger regardless of observer
    /// order. The Surface entity *is* its own host, so no registry mapping is
    /// needed.
    fn install_active_terminal_surface(app: &mut App) -> Entity {
        app.world_mut()
            .run_system_once(
                |mux: MultiplexerCommands,
                 attached_workspace: Query<
                    Entity,
                    (With<WorkspaceMarker>, With<AttachedWorkspace>),
                >| {
                    let workspace = attached_workspace.iter().next()?;
                    let pane = mux.workspaces_active_pane(workspace)?;
                    mux.panes_active_surface(pane)
                },
            )
            .unwrap()
            .unwrap()
    }

    /// Attaches a real `TerminalHandle` / `PtyHandle` / `Coalescer` (via
    /// `TerminalBundle::spawn`) onto the active pane's Surface entity. Used by
    /// the paste-gate integration tests that need to observe
    /// `pending_user_input` flipping after the gate runs.
    fn install_active_terminal_surface_with_handle(app: &mut App) -> Entity {
        let opts = bevy_terminal::SpawnOptions {
            cols: 10,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
            osc_webview_gate: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let bundle = bevy_terminal::TerminalBundle::spawn(opts).expect("spawn /bin/sh");
        let surface = install_active_terminal_surface(app);
        app.world_mut().entity_mut(surface).insert(bundle);
        surface
    }

    /// Spawns an `InlineWebview` child of `surface` and points the
    /// `FocusedWebview` resource at it, mirroring the click-to-focus path.
    fn spawn_focused_inline_child(app: &mut App, surface: Entity) -> Entity {
        let child = app
            .world_mut()
            .spawn((
                ChildOf(surface),
                crate::inline_webview::InlineWebview {
                    view_id: "inline-test".into(),
                    slot: 0,
                },
            ))
            .id();
        app.insert_resource(FocusedWebview(Some(child)));
        child
    }

    /// Feeds enough lines into the surface's VT emulator to grow scrollback,
    /// then scrolls the viewport up so `is_at_bottom()` turns false.
    fn scroll_surface_back(app: &mut App, surface: Entity) {
        app.world_mut()
            .run_system_once(
                move |mut q: Query<(
                    &mut bevy_terminal::TerminalHandle,
                    &mut bevy_terminal::Coalescer,
                )>| {
                    let (mut handle, mut coalescer) = q.get_mut(surface).unwrap();
                    for _ in 0..30 {
                        handle.advance(b"x\r\n");
                    }
                    handle.scroll(&mut coalescer, 3);
                },
            )
            .unwrap();
        assert!(
            !is_at_bottom(app, surface),
            "precondition: viewport must be scrolled back"
        );
    }

    fn is_at_bottom(app: &App, surface: Entity) -> bool {
        app.world()
            .get::<bevy_terminal::TerminalHandle>(surface)
            .unwrap()
            .is_at_bottom()
    }

    fn press_ctrl_shift(app: &mut App) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::ShiftLeft);
    }

    fn release_ctrl_shift(app: &mut App) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::ControlLeft);
        keys.release(KeyCode::ShiftLeft);
    }

    #[test]
    fn bevy_to_configs_key_lowercases_ascii_alphabet() {
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("S".into())),
            Some(CKey::Char('s'))
        );
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("s".into())),
            Some(CKey::Char('s'))
        );
    }

    #[test]
    fn bevy_to_configs_key_normalizes_shift_symbols() {
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("&".into())),
            Some(CKey::Char('7'))
        );
        assert_eq!(
            bevy_to_configs_key(&Bk::Character("{".into())),
            Some(CKey::Char('['))
        );
    }

    #[test]
    fn bevy_to_configs_key_rejects_multichar_payload() {
        assert_eq!(bevy_to_configs_key(&Bk::Character("ab".into())), None);
    }

    #[test]
    fn bevy_to_configs_key_maps_named_keys() {
        assert_eq!(bevy_to_configs_key(&Bk::Escape), Some(CKey::Escape));
        assert_eq!(bevy_to_configs_key(&Bk::Enter), Some(CKey::Enter));
        assert_eq!(bevy_to_configs_key(&Bk::ArrowUp), Some(CKey::ArrowUp));
        assert_eq!(bevy_to_configs_key(&Bk::Tab), Some(CKey::Tab));
    }

    #[test]
    fn bevy_to_configs_key_returns_none_for_modifier_and_f_keys() {
        assert_eq!(bevy_to_configs_key(&Bk::Shift), None);
        assert_eq!(bevy_to_configs_key(&Bk::Control), None);
        assert_eq!(bevy_to_configs_key(&Bk::F1), None);
    }

    #[test]
    fn bevy_to_configs_key_normalizes_shifted_left_bracket() {
        use ozmux_configs::shortcuts::Key as CKey;
        let k = bevy_to_configs_key(&bevy::input::keyboard::Key::Character("{".into()));
        assert_eq!(k, Some(CKey::Char('[')));
    }

    #[test]
    fn bevy_to_configs_key_normalizes_shifted_right_bracket() {
        use ozmux_configs::shortcuts::Key as CKey;
        let k = bevy_to_configs_key(&bevy::input::keyboard::Key::Character("}".into()));
        assert_eq!(k, Some(CKey::Char(']')));
    }

    #[test]
    fn bevy_to_configs_key_maps_plus_character_to_key_plus() {
        use ozmux_configs::shortcuts::Key as CKey;
        let k = bevy_to_configs_key(&bevy::input::keyboard::Key::Character("+".into()));
        assert_eq!(k, Some(CKey::Plus));
    }

    #[test]
    fn is_modifier_only_key_detects_held_modifiers_only() {
        assert!(is_modifier_only_key(&Bk::Shift));
        assert!(is_modifier_only_key(&Bk::Control));
        assert!(is_modifier_only_key(&Bk::Alt));
        assert!(is_modifier_only_key(&Bk::Super));
        assert!(is_modifier_only_key(&Bk::Meta));
        assert!(is_modifier_only_key(&Bk::Hyper));
        assert!(is_modifier_only_key(&Bk::AltGraph));
        assert!(is_modifier_only_key(&Bk::Fn));
        assert!(is_modifier_only_key(&Bk::Symbol));
        assert!(
            !is_modifier_only_key(&Bk::CapsLock),
            "CapsLock is a toggle press, not a held modifier"
        );
        assert!(!is_modifier_only_key(&Bk::NumLock));
        assert!(!is_modifier_only_key(&Bk::ScrollLock));
        assert!(!is_modifier_only_key(&Bk::FnLock));
        assert!(!is_modifier_only_key(&Bk::SymbolLock));
        assert!(!is_modifier_only_key(&Bk::Character("a".into())));
        assert!(!is_modifier_only_key(&Bk::F1));
    }

    #[test]
    fn cmd_c_triggers_copy_to_clipboard_event() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedClipboardOps::default());
        app.add_observer(capture_copy_op);
        install_active_terminal_surface(&mut app);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("c".into()));
        app.update();
        let ops = app
            .world()
            .resource::<CapturedClipboardOps>()
            .0
            .lock()
            .unwrap();
        assert_eq!(
            *ops,
            vec!["copy"],
            "Cmd+C must trigger exactly one CopyToClipboardActionEvent"
        );
    }

    #[test]
    fn cmd_c_in_copy_mode_does_not_trigger_copy_event() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedClipboardOps::default());
        app.add_observer(capture_copy_op);
        let surface_entity = install_active_terminal_surface(&mut app);
        app.world_mut()
            .entity_mut(surface_entity)
            .insert(crate::ui::copy_mode::CopyModeState);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("c".into()));
        app.update();
        let ops = app
            .world()
            .resource::<CapturedClipboardOps>()
            .0
            .lock()
            .unwrap();
        assert!(
            ops.is_empty(),
            "Cmd+C in copy mode must be swallowed by the copy-mode gate, not trigger Copy",
        );
    }

    #[test]
    fn copy_then_paste_same_tick_fire_observers_in_order() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedClipboardOps::default());
        app.add_observer(capture_copy_op);
        app.add_observer(capture_paste_op);
        install_active_terminal_surface(&mut app);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("c".into()));
        press(&mut app, window_entity, Bk::Character("v".into()));
        app.update();
        let ops = app
            .world()
            .resource::<CapturedClipboardOps>()
            .0
            .lock()
            .unwrap();
        assert_eq!(
            *ops,
            vec!["copy", "paste"],
            "same-tick Cmd+C then Cmd+V must fire Copy then Paste"
        );
    }

    #[test]
    fn paste_then_copy_same_tick_fire_observers_in_order() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedClipboardOps::default());
        app.add_observer(capture_copy_op);
        app.add_observer(capture_paste_op);
        install_active_terminal_surface(&mut app);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("v".into()));
        press(&mut app, window_entity, Bk::Character("c".into()));
        app.update();
        let ops = app
            .world()
            .resource::<CapturedClipboardOps>()
            .0
            .lock()
            .unwrap();
        assert_eq!(
            *ops,
            vec!["paste", "copy"],
            "same-tick Cmd+V then Cmd+C must fire Paste then Copy"
        );
    }

    #[test]
    fn shortcut_plugin_registers_systems_without_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(crate::configs::OzmuxConfigsPlugin)
            .add_plugins(OzmuxShortcutPlugin);
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.init_resource::<crate::input::ime::ImeState>();
        app.insert_resource(crate::clipboard::Clipboard::new());
        app.add_message::<KeyboardInput>();
        app.update();
    }

    #[test]
    fn unbound_key_forwards_to_terminal() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let _surface_entity = install_active_terminal_surface(&mut app);

        press(&mut app, window_entity, Bk::Character("h".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "single 'h' must produce exactly one TerminalKeyInput"
        );
        assert!(
            matches!(&captured[0].key, bevy_terminal::TerminalKey::Text(s) if s == "h"),
            "captured key was {:?}",
            captured[0].key
        );
    }

    #[test]
    fn enter_forwards_as_terminal_enter() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_surface(&mut app);

        press(&mut app, window_entity, Bk::Enter);
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(matches!(captured[0].key, bevy_terminal::TerminalKey::Enter));
    }

    #[test]
    fn no_active_terminal_entity_means_no_panic_just_silent_drop() {
        use ozmux_multiplexer::{ActiveSurface, PaneMarker};
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);

        // Strip every pane's ActiveSurface so the dispatcher resolves no focused
        // surface entity — keys must silently drop rather than panic.
        app.world_mut()
            .run_system_once(
                |mut commands: Commands, panes: Query<Entity, With<PaneMarker>>| {
                    for pane in panes.iter() {
                        commands.entity(pane).remove::<ActiveSurface>();
                    }
                },
            )
            .unwrap();
        app.world_mut().flush();

        press(&mut app, window_entity, Bk::Character("h".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(captured.is_empty(), "no active surface → no trigger");
    }

    #[test]
    fn key_consumed_by_copy_mode_gate_does_not_reach_terminal() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let surface_entity = install_active_terminal_surface(&mut app);
        app.world_mut()
            .entity_mut(surface_entity)
            .insert(crate::ui::copy_mode::CopyModeState);

        press(&mut app, window_entity, Bk::Character("h".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "active CopyModeState must consume keys before terminal-forward (captured: {:?})",
            captured,
        );
    }

    #[test]
    fn keys_after_y_in_same_frame_reach_terminal_not_copy_mode() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let surface_entity = install_active_terminal_surface(&mut app);
        app.world_mut()
            .entity_mut(surface_entity)
            .insert(crate::ui::copy_mode::CopyModeState);

        press(&mut app, window_entity, Bk::Character("y".into()));
        press(&mut app, window_entity, Bk::Character("a".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "expected exactly one TerminalKeyInput (for 'a' after 'y' exited copy mode), captured: {:?}",
            captured,
        );
        assert!(
            matches!(&captured[0].key, bevy_terminal::TerminalKey::Text(s) if s == "a"),
            "captured key was {:?}, expected Text(\"a\")",
            captured[0].key,
        );
    }

    #[test]
    fn cmd_v_with_bracketed_paste_off_writes_to_pty_and_consumes_key() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let _surface_entity = install_active_terminal_surface_with_handle(&mut app);
        {
            let mut clipboard = app
                .world_mut()
                .resource_mut::<crate::clipboard::Clipboard>();
            clipboard.write("hello\nworld".to_string());
        }

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("v".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "Cmd+V must NOT forward as TerminalKeyInput; captured: {:?}",
            captured,
        );
    }

    #[test]
    fn cmd_v_with_bracketed_paste_on_wraps_bytes_when_clipboard_has_text() {
        let (mut app, window_entity) = make_app(true);
        let surface_entity = install_active_terminal_surface_with_handle(&mut app);
        let clipboard_available = {
            let cb = app.world().resource::<crate::clipboard::Clipboard>();
            cb.is_available_for_test()
        };
        if !clipboard_available {
            eprintln!("skipping: arboard unavailable in this environment (e.g. headless CI)");
            return;
        }
        {
            let mut clipboard = app
                .world_mut()
                .resource_mut::<crate::clipboard::Clipboard>();
            clipboard.write("hi".to_string());
        }
        {
            let mut handle = app
                .world_mut()
                .get_mut::<bevy_terminal::TerminalHandle>(surface_entity)
                .unwrap();
            handle.advance(b"\x1b[?2004h");
        }
        assert!(
            !app.world()
                .get::<bevy_terminal::TerminalHandle>(surface_entity)
                .unwrap()
                .pending_user_input(),
            "precondition: no pending input before paste",
        );

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("v".into()));
        app.update();

        assert!(
            app.world()
                .get::<bevy_terminal::TerminalHandle>(surface_entity)
                .unwrap()
                .pending_user_input(),
            "after Cmd+V with bracketed paste on and seeded clipboard, handle.write must have been called (flipping pending_user_input to true)",
        );
    }

    #[test]
    fn cmd_v_in_copy_mode_does_not_invoke_paste() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let surface_entity = install_active_terminal_surface_with_handle(&mut app);
        app.world_mut()
            .entity_mut(surface_entity)
            .insert(crate::ui::copy_mode::CopyModeState);
        assert!(
            !app.world()
                .get::<bevy_terminal::TerminalHandle>(surface_entity)
                .unwrap()
                .pending_user_input()
        );

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("v".into()));
        app.update();

        assert!(
            !app.world()
                .get::<bevy_terminal::TerminalHandle>(surface_entity)
                .unwrap()
                .pending_user_input(),
            "copy-mode gate must consume Cmd+V before the paste gate; no write should occur",
        );
        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "Cmd+V in copy mode must not leak to the terminal",
        );
    }

    #[test]
    fn cmd_v_then_next_key_reaches_terminal() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_surface_with_handle(&mut app);

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("v".into()));
        app.update();

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("a".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.iter().any(|ev| matches!(
                &ev.key,
                bevy_terminal::TerminalKey::Text(s) if s == "a"
            )),
            "after Cmd+V, the next plain 'a' must forward to the terminal; captured: {:?}",
            captured,
        );
    }

    #[test]
    fn direct_dispatch_cmd_j_fires_focus_pane_down() {
        let (mut app, window_entity) = make_app(true);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("j".into()));
        app.update();
        let count = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .count();
        assert!(count > 0);
    }

    #[test]
    fn key_repeat_event_is_ignored() {
        let (mut app, window_entity) = make_app(true);
        install_active_terminal_surface(&mut app);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        let ev = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified),
            logical_key: Bk::Character("d".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: true,
            window: window_entity,
        };
        let mut events = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>();
        events.write(ev);
        _ = events;
        app.update();
        let count = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .count();
        assert!(count > 0);
    }

    #[test]
    fn unbound_chord_falls_through_to_terminal_passthrough() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_surface(&mut app);
        press(&mut app, window_entity, Bk::Character("a".into()));
        app.update();
        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.iter().any(|ev| matches!(
                &ev.key,
                bevy_terminal::TerminalKey::Text(s) if s == "a"
            )),
            "plain 'a' must forward to the terminal; captured: {:?}",
            captured,
        );
    }

    #[test]
    fn key_repeat_event_forwards_to_terminal() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_surface(&mut app);
        let ev = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified),
            logical_key: Bk::Character("j".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: true,
            window: window_entity,
        };
        let mut events = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>();
        events.write(ev);
        _ = events;
        app.update();
        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.iter().any(|ev| matches!(
                &ev.key,
                bevy_terminal::TerminalKey::Text(s) if s == "j"
            )),
            "repeat=true 'j' must still forward to the terminal; captured: {:?}",
            captured,
        );
    }

    #[derive(Debug, Default, Resource)]
    struct CapturedActionEvents(Vec<&'static str>);

    fn cap_split(_: On<SplitPaneActionEvent>, mut c: ResMut<CapturedActionEvents>) {
        c.0.push("SplitPane");
    }
    fn cap_new_surface(_: On<NewTerminalSurfaceActionEvent>, mut c: ResMut<CapturedActionEvents>) {
        c.0.push("NewTerminalSurface");
    }
    fn cap_focus_pane(_: On<FocusPaneActionEvent>, mut c: ResMut<CapturedActionEvents>) {
        c.0.push("FocusPane");
    }
    fn cap_focus_surface(_: On<FocusSurfaceActionEvent>, mut c: ResMut<CapturedActionEvents>) {
        c.0.push("FocusSurface");
    }
    fn cap_swap(_: On<SwapPaneActionEvent>, mut c: ResMut<CapturedActionEvents>) {
        c.0.push("SwapPane");
    }
    fn cap_close_pane(_: On<ClosePaneActionEvent>, mut c: ResMut<CapturedActionEvents>) {
        c.0.push("ClosePane");
    }
    fn cap_close_surface(_: On<CloseSurfaceActionEvent>, mut c: ResMut<CapturedActionEvents>) {
        c.0.push("CloseSurface");
    }
    fn cap_new_workspace(_: On<NewWorkspaceActionEvent>, mut c: ResMut<CapturedActionEvents>) {
        c.0.push("NewWorkspace");
    }
    fn cap_focus_workspace(_: On<FocusWorkspaceActionEvent>, mut c: ResMut<CapturedActionEvents>) {
        c.0.push("FocusWorkspace");
    }

    fn setup_exec_app() -> App {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.init_resource::<CapturedActionEvents>();
        app.add_observer(cap_split);
        app.add_observer(cap_new_surface);
        app.add_observer(cap_focus_pane);
        app.add_observer(cap_focus_surface);
        app.add_observer(cap_swap);
        app.add_observer(cap_close_pane);
        app.add_observer(cap_close_surface);
        app.add_observer(cap_new_workspace);
        app.add_observer(cap_focus_workspace);
        app
    }

    fn exec_bootstrap_workspace(world: &mut World) -> Entity {
        world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("test".into())).workspace
            })
            .unwrap()
    }

    fn run_execute_action(app: &mut App, action: ShortcutAction, workspace: Entity) {
        app.world_mut()
            .run_system_once(move |mut commands: Commands, mux: MultiplexerCommands| {
                execute_action(None, &mut commands, &mux, action.clone(), workspace);
            })
            .unwrap();
        app.world_mut().flush();
    }

    fn captured_actions(app: &App) -> Vec<&'static str> {
        app.world().resource::<CapturedActionEvents>().0.clone()
    }

    #[test]
    fn execute_action_split_pane_triggers_split_pane_action_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(
            &mut app,
            ShortcutAction::SplitPane {
                direction: ozmux_configs::shortcuts::SplitDirection::Horizontal,
            },
            workspace,
        );
        assert_eq!(captured_actions(&app), vec!["SplitPane"]);
    }

    #[test]
    fn execute_action_new_terminal_surface_triggers_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(&mut app, ShortcutAction::NewTerminalSurface, workspace);
        assert_eq!(captured_actions(&app), vec!["NewTerminalSurface"]);
    }

    #[test]
    fn execute_action_focus_pane_triggers_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(
            &mut app,
            ShortcutAction::FocusPane {
                direction: ozmux_configs::shortcuts::Direction::Right,
            },
            workspace,
        );
        assert_eq!(captured_actions(&app), vec!["FocusPane"]);
    }

    #[test]
    fn execute_action_focus_surface_next_triggers_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(
            &mut app,
            ShortcutAction::FocusSurface {
                offset: ozmux_configs::shortcuts::SurfaceOffset::Next,
            },
            workspace,
        );
        assert_eq!(captured_actions(&app), vec!["FocusSurface"]);
    }

    #[test]
    fn execute_action_focus_surface_last_emits_no_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(
            &mut app,
            ShortcutAction::FocusSurface {
                offset: ozmux_configs::shortcuts::SurfaceOffset::Last,
            },
            workspace,
        );
        assert!(captured_actions(&app).is_empty());
    }

    #[test]
    fn execute_action_swap_pane_triggers_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(
            &mut app,
            ShortcutAction::SwapPane {
                offset: ozmux_configs::shortcuts::SwapOffset::Prev,
            },
            workspace,
        );
        assert_eq!(captured_actions(&app), vec!["SwapPane"]);
    }

    #[test]
    fn execute_action_close_pane_triggers_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(&mut app, ShortcutAction::ClosePane, workspace);
        assert_eq!(captured_actions(&app), vec!["ClosePane"]);
    }

    #[test]
    fn execute_action_close_surface_triggers_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(&mut app, ShortcutAction::CloseSurface, workspace);
        assert_eq!(captured_actions(&app), vec!["CloseSurface"]);
    }

    #[test]
    fn execute_action_new_workspace_triggers_new_workspace_action_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(&mut app, ShortcutAction::NewWorkspace, workspace);
        assert_eq!(captured_actions(&app), vec!["NewWorkspace"]);
    }

    #[test]
    fn execute_action_focus_workspace_triggers_focus_workspace_action_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(
            &mut app,
            ShortcutAction::FocusWorkspace {
                offset: WorkspaceOffset::Next,
            },
            workspace,
        );
        assert_eq!(captured_actions(&app), vec!["FocusWorkspace"]);
    }

    #[test]
    fn execute_action_focus_workspace_number_triggers_focus_workspace_action_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(
            &mut app,
            ShortcutAction::FocusWorkspaceNumber { index: 0 },
            workspace,
        );
        assert_eq!(captured_actions(&app), vec!["FocusWorkspace"]);
    }

    #[test]
    fn execute_action_unimplemented_emits_no_event() {
        let mut app = setup_exec_app();
        let workspace = exec_bootstrap_workspace(app.world_mut());
        run_execute_action(&mut app, ShortcutAction::ZoomPane, workspace);
        assert!(captured_actions(&app).is_empty());
    }

    #[test]
    fn execute_action_on_vanished_workspace_triggers_without_panic() {
        let mut app = setup_exec_app();
        let bogus = app.world_mut().spawn(WorkspaceMarker).id();
        app.world_mut().despawn(bogus);
        app.world_mut().flush();
        run_execute_action(&mut app, ShortcutAction::ClosePane, bogus);
        assert_eq!(captured_actions(&app), vec!["ClosePane"]);
    }

    #[test]
    fn dispatch_focused_key_suppressed_during_composition() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        install_active_terminal_surface(&mut app);

        {
            let mut state = app
                .world_mut()
                .resource_mut::<crate::input::ime::ImeState>();
            crate::input::ime::apply_event(
                &mut state,
                &bevy::window::Ime::Preedit {
                    window: Entity::PLACEHOLDER,
                    value: "あ".into(),
                    cursor: Some((3, 3)),
                },
            );
        }

        press(&mut app, window_entity, Bk::Character("a".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "dispatch_focused_key must suppress keys while ImeState is composing; captured: {:?}",
            captured,
        );
    }

    #[test]
    fn release_chord_clears_inline_focus_even_when_scrolled_back() {
        let (mut app, window_entity) = make_app(true);
        let surface = install_active_terminal_surface_with_handle(&mut app);
        scroll_surface_back(&mut app, surface);
        spawn_focused_inline_child(&mut app, surface);

        press_ctrl_shift(&mut app);
        press(&mut app, window_entity, Bk::Escape);
        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "Ctrl+Shift+Escape must clear inline focus even while scrolled back"
        );
        assert!(
            !is_at_bottom(&app, surface),
            "the release chord must NOT snap the viewport to the bottom"
        );
    }

    #[test]
    fn release_chord_works_during_copy_mode() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let surface = install_active_terminal_surface_with_handle(&mut app);
        app.world_mut()
            .entity_mut(surface)
            .insert(crate::ui::copy_mode::CopyModeState);
        spawn_focused_inline_child(&mut app, surface);

        press_ctrl_shift(&mut app);
        press(&mut app, window_entity, Bk::Escape);
        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "Ctrl+Shift+Escape must clear inline focus even while copy mode is active"
        );
        assert!(
            app.world()
                .get::<crate::ui::copy_mode::CopyModeState>(surface)
                .is_some(),
            "the hoisted chord check must consume the event before copy mode's Escape arm"
        );

        release_ctrl_shift(&mut app);
        press(&mut app, window_entity, Bk::Character("x".into()));
        app.update();

        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "copy mode must still swallow ordinary keys after the chord released focus; captured: {:?}",
            captured,
        );
    }

    #[test]
    fn focused_inline_webview_wins_over_copy_mode_for_shortcuts() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedClipboardOps::default());
        app.add_observer(capture_copy_op);
        let surface = install_active_terminal_surface(&mut app);
        app.world_mut()
            .entity_mut(surface)
            .insert(crate::ui::copy_mode::CopyModeState);
        spawn_focused_inline_child(&mut app, surface);

        // Copy mode normally SWALLOWS Cmd+C (see
        // `cmd_c_in_copy_mode_does_not_trigger_copy_event`). With an inline
        // webview focused the copy-mode gate must be skipped so global
        // shortcuts still fire — the firing Copy is what distinguishes the
        // fixed precedence (focused inline wins over copy mode, matching the
        // wheel path) from the pre-fix behavior, where copy mode ate the chord.
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("c".into()));
        app.update();

        let ops = app
            .world()
            .resource::<CapturedClipboardOps>()
            .0
            .lock()
            .unwrap();
        assert_eq!(
            *ops,
            vec!["copy"],
            "a focused inline webview must let the Cmd+C shortcut fire past the copy-mode gate"
        );
    }

    #[test]
    fn release_chord_repeat_is_consumed_without_clearing_focus() {
        let (mut app, window_entity) = make_app(true);
        let surface = install_active_terminal_surface_with_handle(&mut app);
        let child = spawn_focused_inline_child(&mut app, surface);

        press_ctrl_shift(&mut app);
        let ev = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified),
            logical_key: Bk::Escape,
            state: ButtonState::Pressed,
            text: None,
            repeat: true,
            window: window_entity,
        };
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<KeyboardInput>>()
            .write(ev);
        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "a repeat release-chord event must be consumed without clearing focus"
        );
    }

    #[test]
    fn unmodified_escape_still_snaps_to_bottom_without_inline_focus() {
        let (mut app, window_entity) = make_app(true);
        let surface = install_active_terminal_surface_with_handle(&mut app);
        scroll_surface_back(&mut app, surface);

        press(&mut app, window_entity, Bk::Escape);
        app.update();

        assert!(
            is_at_bottom(&app, surface),
            "plain Escape while scrolled back (no inline focus) must snap to bottom"
        );
    }

    #[test]
    fn modified_escape_skips_the_escape_pre_handler() {
        let (mut app, window_entity) = make_app(true);
        let surface = install_active_terminal_surface_with_handle(&mut app);
        scroll_surface_back(&mut app, surface);

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::AltLeft);
        }
        press(&mut app, window_entity, Bk::Escape);
        app.update();

        assert!(
            !is_at_bottom(&app, surface),
            "Alt+Escape must not trigger the scroll-to-bottom Escape pre-handler"
        );
    }

    #[test]
    fn escape_while_inline_focused_neither_snaps_nor_forwards() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        let surface = install_active_terminal_surface_with_handle(&mut app);
        scroll_surface_back(&mut app, surface);
        let child = spawn_focused_inline_child(&mut app, surface);

        press(&mut app, window_entity, Bk::Escape);
        app.update();

        assert!(
            !is_at_bottom(&app, surface),
            "plain Escape typed into a focused page must not snap the viewport"
        );
        let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "plain Escape while inline-focused must not forward to the PTY; captured: {:?}",
            captured,
        );
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "plain Escape must not release inline focus"
        );
    }

    #[test]
    fn keys_suppressed_to_pty_while_inline_focused_but_shortcuts_fire() {
        let (mut app, window_entity) = make_app(true);
        app.insert_resource(CapturedKeys::default());
        app.add_observer(capture_key_input);
        app.insert_resource(CapturedClipboardOps::default());
        app.add_observer(capture_copy_op);
        let surface = install_active_terminal_surface_with_handle(&mut app);
        let child = spawn_focused_inline_child(&mut app, surface);

        press(&mut app, window_entity, Bk::Character("h".into()));
        app.update();
        {
            let captured = app.world().resource::<CapturedKeys>().0.lock().unwrap();
            assert!(
                captured.is_empty(),
                "printable keys must not reach the PTY while an inline webview holds focus; captured: {:?}",
                captured,
            );
        }

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::SuperLeft);
        }
        press(&mut app, window_entity, Bk::Character("c".into()));
        app.update();

        let ops = app
            .world()
            .resource::<CapturedClipboardOps>()
            .0
            .lock()
            .unwrap();
        assert_eq!(
            *ops,
            vec!["copy"],
            "bound global shortcuts must still execute while inline-focused"
        );
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "Cmd+C must not clear inline focus"
        );
    }
}
