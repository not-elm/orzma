//! Startup tmux session picker: lists sessions, shows a keyboard-navigable
//! overlay, and attaches only after the user selects an entry (or "New session").

use crate::configs::OzmuxConfigsResource;
use crate::control_plane::ControlPlaneHandle;
use crate::ozma::AppMode;
use crate::theme;
use bevy::ecs::hierarchy::Children;
use bevy::ecs::message::MessageReader;
use bevy::ecs::schedule::common_conditions::{not, resource_exists_and_changed};
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, ScrollPosition, UiGlobalTransform, UiSystems};
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use ozmux_configs::StartupMode;
use ozmux_tmux::{
    AttachTarget, ConnectionState, TmuxConnection, attach_or_create, select_attach_target,
    select_window_command, set_environment_in_session_command, switch_client_command,
};
use tmux_control::{SessionInfo, TmuxServer, WindowEntry};

const PICKER_Z: i32 = 310;

/// Registers the session picker UI and keyboard handler.
pub(crate) struct OzmuxPickerPlugin;

impl Plugin for OzmuxPickerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SessionPicker>()
            .add_systems(Startup, spawn_picker_ui)
            .add_systems(
                OnEnter(AppMode::Ozmux),
                dispatch_startup_mode.run_if(not(resource_exists::<StartupDispatched>)),
            )
            .add_systems(
                Update,
                handle_picker_input.after(crate::input::InputPhase::FocusedKey),
            )
            .add_systems(Update, refresh_picker_on_open)
            .add_systems(
                Update,
                refresh_session_ozmux_sock
                    .run_if(resource_exists_and_changed::<ConnectionState>)
                    .in_set(crate::tmux::OzmuxActiveSet),
            )
            .add_systems(
                Last,
                cleanup_session_ozmux_sock.run_if(on_message::<AppExit>),
            )
            .add_systems(
                PostUpdate,
                sync_picker_ui
                    .before(UiSystems::Layout)
                    .run_if(resource_exists_and_changed::<SessionPicker>),
            )
            .add_systems(
                PostUpdate,
                scroll_selected_into_view
                    .after(UiSystems::Layout)
                    .run_if(picker_is_open)
                    .run_if(resource_exists_and_changed::<SessionPicker>),
            )
            .add_systems(
                Update,
                handle_picker_row_interaction.run_if(picker_is_open),
            )
            .add_systems(
                Update,
                picker_row_hover_cursor
                    .after(crate::input::InputPhase::Hover)
                    .run_if(picker_is_open),
            )
            .add_systems(
                Update,
                handle_picker_scroll
                    .run_if(on_message::<MouseWheel>)
                    .run_if(picker_is_open),
            );
    }
}

/// One selectable row in the chooser tree: a session header, a window under a
/// session (indices into the picker's `sessions` / `windows`), or the trailing
/// "New session" entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerRow {
    Session(usize),
    Window { session: usize, window: usize },
    NewSession,
}

#[derive(Resource, Default)]
pub(crate) struct SessionPicker {
    sessions: Vec<SessionInfo>,
    windows: Vec<WindowEntry>,
    selected: usize,
    pub(crate) open: bool,
    last_open: bool,
}

/// Inserted on the first `OnEnter(AppMode::Ozmux)` (boot) so the startup-mode
/// dispatch routes only once and later Ozmux entries skip it.
#[derive(Resource)]
struct StartupDispatched;

#[derive(Component)]
struct PickerBackdrop;

#[derive(Component)]
struct PickerList;

#[derive(Component)]
struct PickerRowLabel(usize);

fn build_rows(sessions: &[SessionInfo], windows: &[WindowEntry]) -> Vec<PickerRow> {
    let mut rows = Vec::new();
    for (si, session) in sessions.iter().enumerate() {
        rows.push(PickerRow::Session(si));
        for (wi, window) in windows.iter().enumerate() {
            if window.session_id == session.id {
                rows.push(PickerRow::Window {
                    session: si,
                    window: wi,
                });
            }
        }
    }
    rows.push(PickerRow::NewSession);
    rows
}

fn build_server(configs: &OzmuxConfigsResource) -> TmuxServer {
    let cfg = &configs.tmux;
    let mut server = TmuxServer::new().program(&cfg.program);
    if let Some(name) = &cfg.socket_name {
        server = server.socket_name(name);
    }
    server
}

/// Refreshes the chooser's session + window lists on the closed→open edge via
/// one-shot `TmuxServer` subprocess queries against the same socket. The
/// subprocess sees the live server whether or not a control client is attached,
/// mirroring the boot path — so the chooser needs no control-mode reply
/// correlation.
fn refresh_picker_on_open(mut picker: ResMut<SessionPicker>, configs: Res<OzmuxConfigsResource>) {
    let opened = picker.open && !picker.last_open;
    if picker.last_open != picker.open {
        picker.last_open = picker.open;
    }
    if !opened {
        return;
    }
    let server = build_server(&configs);
    match (server.list_sessions(), server.list_windows_all()) {
        (Ok(sessions), Ok(windows)) => {
            picker.sessions = sessions;
            picker.windows = windows;
            picker.selected = 0;
        }
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(?e, "failed to refresh session chooser");
        }
    }
}

/// Boot-time startup-mode routing, run once. Registered on `OnEnter(Ozmux)`
/// behind `run_if(not(resource_exists::<StartupDispatched>))`; it inserts
/// `StartupDispatched` on its first (boot) run so later Ozmux entries skip it
/// and do not bounce a picker-driven Ozma -> Ozmux transition back to Ozma.
fn dispatch_startup_mode(
    mut commands: Commands,
    mut connection: NonSendMut<TmuxConnection>,
    mut picker: ResMut<SessionPicker>,
    mut state: ResMut<ConnectionState>,
    mut next_mode: ResMut<NextState<AppMode>>,
    configs: Res<OzmuxConfigsResource>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    commands.insert_resource(StartupDispatched);
    match &configs.startup_mode {
        StartupMode::Ozma => {
            next_mode.set(AppMode::Ozma);
        }
        StartupMode::Ozmux => {
            let server = build_server(&configs);
            match server.list_sessions() {
                Ok(sessions) => {
                    picker.sessions = sessions;
                    picker.selected = 0;
                    picker.open = true;
                }
                Err(e) => {
                    *state = ConnectionState::Error {
                        reason: format!("tmux unavailable: {e}"),
                    };
                }
            }
        }
        StartupMode::AutoAttach => {
            let mut server = build_server(&configs);
            if let Some(handle) = &control {
                server = server.env("OZMA_SOCK", &handle.sock_path.to_string_lossy());
            }
            match server.list_sessions() {
                Ok(sessions) => {
                    let target = select_attach_target(&sessions);
                    match attach_or_create(&server, &target) {
                        Ok(client) => {
                            connection.set(client);
                            *state = ConnectionState::Connecting;
                        }
                        Err(e) => {
                            *state = ConnectionState::Error {
                                reason: format!("auto-attach failed: {e}"),
                            };
                        }
                    }
                }
                Err(e) => {
                    *state = ConnectionState::Error {
                        reason: format!("tmux unavailable: {e}"),
                    };
                }
            }
        }
    }
}

fn spawn_picker_ui(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::None,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.8)),
            GlobalZIndex(PICKER_Z),
            PickerBackdrop,
        ))
        .with_children(|parent| {
            parent
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        min_width: Val::Px(360.0),
                        max_height: Val::Vh(65.0),
                        padding: UiRect::axes(Val::Px(20.0), Val::Px(16.0)),
                        row_gap: Val::Px(10.0),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(8.0)),
                        ..default()
                    },
                    BackgroundColor(theme::TAB_BAR_BG),
                    BorderColor::all(theme::BORDER),
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new("TMUX SESSIONS"),
                        TextColor(theme::MUTED),
                        TextFont {
                            font_size: theme::PICKER_TITLE_FONT_SIZE_PX,
                            ..default()
                        },
                    ));
                    panel.spawn((
                        Node {
                            flex_direction: FlexDirection::Column,
                            width: Val::Percent(100.0),
                            row_gap: Val::Px(2.0),
                            flex_grow: 1.0,
                            min_height: Val::Px(0.0),
                            overflow: Overflow::scroll_y(),
                            ..default()
                        },
                        ScrollPosition::default(),
                        PickerList,
                    ));
                    panel.spawn((
                        Node {
                            border: UiRect::top(Val::Px(1.0)),
                            padding: UiRect::top(Val::Px(8.0)),
                            ..default()
                        },
                        BorderColor::all(theme::DIVIDER),
                        Text::new("↑↓/jk select · ⏎ open · esc cancel"),
                        TextColor(theme::MUTED),
                        TextFont {
                            font_size: theme::PICKER_TITLE_FONT_SIZE_PX,
                            ..default()
                        },
                    ));
                });
        });
}

/// The label text, foreground color, and highlight-bar color for each row,
/// derived from the picker data and the selected index. The selected row gets
/// the amber bar + dark text; unselected rows are transparent, with window
/// rows muted to read as a level below their session header.
fn row_visuals(
    picker: &SessionPicker,
    rows: &[PickerRow],
    selected: usize,
) -> Vec<(String, Color, Color)> {
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let is_selected = i == selected;
            let (label, base) = match row {
                PickerRow::Session(si) => {
                    let s = &picker.sessions[*si];
                    let attached = if s.attached { " ·attached" } else { "" };
                    (format!("({}) {}{}", i, s.name, attached), theme::FOREGROUND)
                }
                PickerRow::Window { window, .. } => {
                    let w = &picker.windows[*window];
                    let active = if w.window_active { "*" } else { "" };
                    (
                        format!("({}) └ {}: {}{}", i, w.window_index, w.window_name, active),
                        theme::MUTED,
                    )
                }
                PickerRow::NewSession => (format!("({}) + New session", i), theme::MUTED),
            };
            let text_color = if is_selected {
                theme::SELECTION_FG
            } else {
                base
            };
            let bar_color = if is_selected {
                theme::SELECTION
            } else {
                Color::NONE
            };
            (label, text_color, bar_color)
        })
        .collect()
}

fn sync_picker_ui(
    mut backdrop: Query<&mut Node, With<PickerBackdrop>>,
    mut rows_q: Query<(&mut Text, &mut TextColor, &mut BackgroundColor), With<PickerRowLabel>>,
    mut commands: Commands,
    picker: Res<SessionPicker>,
    list_query: Query<(Entity, Option<&Children>), With<PickerList>>,
) {
    let Ok(mut node) = backdrop.single_mut() else {
        return;
    };
    node.display = if picker.open {
        Display::Flex
    } else {
        Display::None
    };

    let Ok((list_entity, children)) = list_query.single() else {
        return;
    };

    let rows = build_rows(&picker.sessions, &picker.windows);
    let selected = picker.selected.min(rows.len().saturating_sub(1));
    let visuals = row_visuals(&picker, &rows, selected);

    let existing: &[Entity] = children.map(|c| &**c).unwrap_or(&[]);
    if existing.len() == visuals.len() {
        // NOTE: update existing row entities in place rather than despawning and
        // respawning — recreating Text nodes on every selection change drops a
        // frame of layout and makes navigation flicker. Guarded writes keep
        // change detection honest.
        for (&entity, (label, text_color, bar_color)) in existing.iter().zip(visuals) {
            let Ok((mut text, mut color, mut bg)) = rows_q.get_mut(entity) else {
                continue;
            };
            if text.0 != label {
                text.0 = label;
            }
            if color.0 != text_color {
                color.0 = text_color;
            }
            if bg.0 != bar_color {
                bg.0 = bar_color;
            }
        }
    } else {
        commands.entity(list_entity).despawn_related::<Children>();
        commands.entity(list_entity).with_children(|parent| {
            for (i, (label, text_color, bar_color)) in visuals.into_iter().enumerate() {
                parent.spawn((
                    Button,
                    Node {
                        width: Val::Percent(100.0),
                        padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        ..default()
                    },
                    Text::new(label),
                    TextColor(text_color),
                    BackgroundColor(bar_color),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    PickerRowLabel(i),
                ));
            }
        });
    }
}

// NOTE: `hover_armed` stays false until a CursorMoved arrives after the picker
// opens, so opening under a stationary cursor does not hijack the keyboard's
// first-session selection. Clicks (Pressed) are never gated by it.
fn handle_picker_row_interaction(
    mut picker: ResMut<SessionPicker>,
    mut connection: NonSendMut<TmuxConnection>,
    mut state: ResMut<ConnectionState>,
    mut next_mode: ResMut<NextState<AppMode>>,
    mut cursor_moved: MessageReader<CursorMoved>,
    mut hover_armed: Local<bool>,
    mut was_open: Local<bool>,
    rows: Query<(&Interaction, &PickerRowLabel), Changed<Interaction>>,
    configs: Res<OzmuxConfigsResource>,
    current_mode: Res<State<AppMode>>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let opened = picker.open && !*was_open;
    *was_open = picker.open;
    if opened {
        *hover_armed = false;
    }
    if cursor_moved.read().count() > 0 {
        *hover_armed = true;
    }

    for (interaction, label) in rows.iter() {
        match interaction {
            Interaction::Pressed => {
                picker.selected = label.0;
                let built = build_rows(&picker.sessions, &picker.windows);
                let row = built
                    .get(picker.selected)
                    .copied()
                    .unwrap_or(PickerRow::NewSession);
                activate_row(
                    &mut connection,
                    &mut state,
                    &mut next_mode,
                    &configs,
                    control.as_deref(),
                    &picker,
                    current_mode.get(),
                    row,
                );
                picker.open = false;
                break;
            }
            Interaction::Hovered => {
                if *hover_armed {
                    picker.selected = label.0;
                }
            }
            Interaction::None => {}
        }
    }
}

fn picker_row_hover_cursor(
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    rows: Query<&Interaction, With<PickerRowLabel>>,
) {
    let hovering = rows
        .iter()
        .any(|i| matches!(i, Interaction::Hovered | Interaction::Pressed));
    let Ok(mut icon) = cursor_icons.single_mut() else {
        return;
    };
    let is_pointer = matches!(&*icon, CursorIcon::System(e) if *e == SystemCursorIcon::Pointer);
    if hovering && !is_pointer {
        *icon = CursorIcon::System(SystemCursorIcon::Pointer);
    } else if !hovering && is_pointer {
        *icon = CursorIcon::System(SystemCursorIcon::Default);
    }
}

fn handle_picker_scroll(
    mut list: Query<(&mut ScrollPosition, &ComputedNode), With<PickerList>>,
    mut wheel: MessageReader<MouseWheel>,
) {
    let Ok((mut pos, node)) = list.single_mut() else {
        wheel.clear();
        return;
    };
    let mut delta = 0.0;
    for ev in wheel.read() {
        delta += wheel_delta_px(ev.unit, ev.y);
    }
    if delta == 0.0 {
        return;
    }
    let inv = node.inverse_scale_factor;
    let max = (node.content_size().y - node.size().y).max(0.0) * inv;
    let next = (pos.0.y + delta).clamp(0.0, max);
    if pos.0.y != next {
        pos.0.y = next;
    }
}

// NOTE: writes ScrollPosition after UiSystems::Layout, so the correction lands on
// the next frame's render. Reads physical-px geometry (ComputedNode size +
// UiGlobalTransform center) and converts to logical via inverse_scale_factor,
// because ScrollPosition is in logical px.
fn scroll_selected_into_view(
    mut list: Query<(&mut ScrollPosition, &ComputedNode, &UiGlobalTransform, &Children), With<PickerList>>,
    rows: Query<(&ComputedNode, &UiGlobalTransform, &PickerRowLabel)>,
    picker: Res<SessionPicker>,
) {
    let Ok((mut pos, list_node, list_tf, children)) = list.single_mut() else {
        return;
    };
    let viewport_h_phys = list_node.size().y;
    if viewport_h_phys <= 0.0 {
        return;
    }

    let mut selected: Option<(f32, f32)> = None;
    for child in children.iter() {
        let Ok((row_node, row_tf, label)) = rows.get(child) else {
            continue;
        };
        if label.0 == picker.selected {
            let h = row_node.size().y;
            let top = row_tf.translation.y - h / 2.0;
            selected = Some((top, h));
            break;
        }
    }
    let Some((row_top_global, row_h_phys)) = selected else {
        return;
    };
    if row_h_phys <= 0.0 {
        return;
    }

    let inv = list_node.inverse_scale_factor;
    let list_top_global = list_tf.translation.y - viewport_h_phys / 2.0;
    let current = pos.0.y;
    let row_top = current + (row_top_global - list_top_global) * inv;
    let row_h = row_h_phys * inv;
    let viewport_h = viewport_h_phys * inv;
    let max = (list_node.content_size().y - viewport_h_phys).max(0.0) * inv;

    let next = reveal_offset(row_top, row_h, viewport_h, current, max);
    if pos.0.y != next {
        pos.0.y = next;
    }
}

fn handle_picker_input(
    mut picker: ResMut<SessionPicker>,
    mut connection: NonSendMut<TmuxConnection>,
    mut state: ResMut<ConnectionState>,
    mut next_mode: ResMut<NextState<AppMode>>,
    mut keys: MessageReader<KeyboardInput>,
    configs: Res<OzmuxConfigsResource>,
    current_mode: Res<State<AppMode>>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    if !picker.open {
        keys.clear();
        return;
    }
    let rows = build_rows(&picker.sessions, &picker.windows);
    let entry_count = rows.len();
    for ev in keys.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match ev.key_code {
            KeyCode::ArrowUp | KeyCode::KeyK => {
                picker.selected = step_selection(picker.selected, entry_count, true);
            }
            KeyCode::ArrowDown | KeyCode::KeyJ => {
                picker.selected = step_selection(picker.selected, entry_count, false);
            }
            KeyCode::Escape => {
                picker.open = false;
                break;
            }
            KeyCode::Enter => {
                let row = rows
                    .get(picker.selected)
                    .copied()
                    .unwrap_or(PickerRow::NewSession);
                activate_row(
                    &mut connection,
                    &mut state,
                    &mut next_mode,
                    &configs,
                    control.as_deref(),
                    &picker,
                    current_mode.get(),
                    row,
                );
                picker.open = false;
                break;
            }
            _ => {}
        }
    }
}

fn picker_is_open(picker: Res<SessionPicker>) -> bool {
    picker.open
}

// NOTE: mirrors the keyboard Enter arm exactly — used by both the Enter handler
// and the mouse click handler so the two paths cannot diverge. Leaves
// `picker.open` to the caller.
fn activate_row(
    connection: &mut TmuxConnection,
    state: &mut ConnectionState,
    next_mode: &mut NextState<AppMode>,
    configs: &OzmuxConfigsResource,
    control: Option<&ControlPlaneHandle>,
    picker: &SessionPicker,
    current_mode: &AppMode,
    row: PickerRow,
) {
    if connection.client().is_some() {
        apply_switch(connection, state, configs, control, picker, row);
    } else {
        let attached = apply_attach(connection, state, configs, control, picker, row);
        if should_enter_ozmux(attached, current_mode) {
            next_mode.set(AppMode::Ozmux);
        }
    }
}

// NOTE: while attached, switching must go through the live control client so the
// single `tmux -CC` connection survives; a fresh `attach_or_create` would spawn a
// second client and orphan the first.
fn apply_switch(
    connection: &mut TmuxConnection,
    state: &mut ConnectionState,
    configs: &OzmuxConfigsResource,
    control: Option<&ControlPlaneHandle>,
    picker: &SessionPicker,
    row: PickerRow,
) {
    if connection.client().is_none() {
        return;
    }
    // The attach path's current-session set-environment does not re-run on a
    // switch, so each switched-to session needs $OZMA_SOCK set explicitly;
    // newly-created sessions get it via `new-session -e` instead.
    let ozma_sock = control.map(|handle| handle.sock_path.to_string_lossy().into_owned());
    let target = match row {
        PickerRow::Session(si) => Some((picker.sessions[si].name.clone(), None)),
        PickerRow::Window { session, window } => Some((
            picker.sessions[session].name.clone(),
            Some(picker.windows[window].window_id),
        )),
        PickerRow::NewSession => None,
    };
    let cmds: Vec<String> = match target {
        Some((name, window)) => {
            // set-environment before switch-client so the target session carries
            // $OZMA_SOCK before any pane is spawned there post-switch; `-t` makes
            // it independent of which session is current.
            let mut cmds = Vec::new();
            if let Some(sock) = &ozma_sock {
                cmds.push(set_environment_in_session_command(&name, "OZMA_SOCK", sock));
            }
            cmds.push(switch_client_command(&name));
            if let Some(window_id) = window {
                cmds.push(select_window_command(window_id));
            }
            cmds
        }
        None => {
            let mut server = build_server(configs);
            if let Some(sock) = &ozma_sock {
                server = server.env("OZMA_SOCK", sock);
            }
            match server.create_detached_session() {
                Ok(name) => vec![switch_client_command(&name)],
                Err(e) => {
                    tracing::warn!(?e, "failed to create new session");
                    return;
                }
            }
        }
    };
    let mut failure: Option<String> = None;
    for cmd in &cmds {
        let Some(client) = connection.client() else {
            return;
        };
        if let Err(e) = client.handle().send(cmd) {
            tracing::warn!(?e, cmd = cmd.as_str(), "switch command send failed");
            failure = Some(format!("switch failed: {e}"));
            break;
        }
    }
    if let Some(reason) = failure {
        connection.take();
        *state = ConnectionState::Error { reason };
    }
}

fn apply_attach(
    connection: &mut TmuxConnection,
    state: &mut ConnectionState,
    configs: &OzmuxConfigsResource,
    control: Option<&ControlPlaneHandle>,
    picker: &SessionPicker,
    row: PickerRow,
) -> bool {
    let target = match row {
        PickerRow::Session(si) => AttachTarget::Attach(picker.sessions[si].name.clone()),
        PickerRow::Window { session, .. } => {
            AttachTarget::Attach(picker.sessions[session].name.clone())
        }
        PickerRow::NewSession => AttachTarget::CreateNew,
    };
    let mut server = build_server(configs);
    if let Some(handle) = control {
        server = server.env("OZMA_SOCK", &handle.sock_path.to_string_lossy());
    }
    match attach_or_create(&server, &target) {
        Ok(client) => {
            connection.set(client);
            *state = ConnectionState::Connecting;
            true
        }
        Err(e) => {
            *state = ConnectionState::Error {
                reason: format!("tmux connect failed: {e}"),
            };
            false
        }
    }
}

/// Whether a successful picker attach should transition the app into
/// [`AppMode::Ozmux`]: true only when the attach succeeded and the app is
/// currently in [`AppMode::Ozma`].
///
/// The `Ozma` guard is correctness-critical, not an optimization: under Bevy
/// 0.18 `NextState::set` to the current value re-runs `OnExit`/`OnEnter`, so
/// setting `Ozmux` while already in `Ozmux` would run `on_exit_ozmux`
/// (`connection.take()` + `detach-client`) and tear down the live client.
fn should_enter_ozmux(attached: bool, current: &AppMode) -> bool {
    attached && *current == AppMode::Ozma
}

/// Refreshes `$OZMA_SOCK` to this ozmux's live control socket across the whole
/// tmux server on attach: session-scoped on every existing session (overwriting
/// any stale value a previously-exited ozmux left behind) plus the server-global
/// environment so sessions created later inherit it. Gated to run on every
/// [`ConnectionState`] change; the body acts only while `Attached`.
///
/// A pre-existing pane never inherited `$OZMA_SOCK` in its own process env, so
/// `ratatui-ozma` recovers it at connect time via `tmux show-environment`; that
/// recovery only works when the stored value points at the live socket. Writes
/// go through one-shot `tmux` subprocesses (not the live `-CC` client) so they
/// are independent of the client and land synchronously. No-op when the control
/// plane is down.
fn refresh_session_ozmux_sock(
    state: Res<ConnectionState>,
    configs: Res<OzmuxConfigsResource>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    if !matches!(*state, ConnectionState::Attached) {
        return;
    }
    let Some(handle) = control else {
        return;
    };
    let sock = handle.sock_path.to_string_lossy().into_owned();
    let server = build_server(&configs);
    let sessions = match server.list_sessions() {
        Ok(sessions) => sessions,
        Err(e) => {
            tracing::warn!(?e, "failed to list sessions while refreshing $OZMA_SOCK");
            return;
        }
    };
    for session in &sessions {
        if let Err(e) = server.run_oneshot(&[
            "set-environment",
            "-t",
            session.name.as_str(),
            "OZMA_SOCK",
            sock.as_str(),
        ]) {
            tracing::warn!(
                ?e,
                session = session.name.as_str(),
                "failed to set $OZMA_SOCK"
            );
        }
    }
    if let Err(e) = server.run_oneshot(&["set-environment", "-g", "OZMA_SOCK", sock.as_str()]) {
        tracing::warn!(?e, "failed to set global $OZMA_SOCK");
    }
}

/// On app exit, removes `$OZMA_SOCK` from the tmux server environment and deletes
/// this process's control runtime dir so a pre-existing pane's `ratatui-ozma`
/// cannot later resolve a dead socket path.
///
/// Gated by `run_if(on_message::<AppExit>)` so it runs only on the exit frame
/// (window close and the in-`DefaultPlugins` `TerminalCtrlCHandlerPlugin` Ctrl-C
/// path both emit `AppExit`). The env unset is best-effort via one-shot `tmux`
/// subprocesses, which flush before the process exits — unlike a write over the
/// `-CC` client, which races its teardown. The runtime dir is removed explicitly
/// here rather than relying solely on [`crate::control_plane`]'s `RuntimeRoot`
/// `Drop`, which can be skipped when noisy CEF teardown ends the process before
/// the world is dropped. A hard kill (SIGKILL / un-handled SIGTERM) skips all of
/// this; the next attach overwrites every value via [`refresh_session_ozmux_sock`].
fn cleanup_session_ozmux_sock(
    configs: Res<OzmuxConfigsResource>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let Some(handle) = control else {
        return;
    };
    let server = build_server(&configs);
    if let Ok(sessions) = server.list_sessions() {
        for session in &sessions {
            let _ = server.run_oneshot(&[
                "set-environment",
                "-t",
                session.name.as_str(),
                "-u",
                "OZMA_SOCK",
            ]);
        }
    }
    let _ = server.run_oneshot(&["set-environment", "-gu", "OZMA_SOCK"]);
    // NOTE: remove the runtime root (`<temp>/<pid>/control`, = sock_path's
    // grandparent) but NOT its `<pid>` parent — sibling webview runtime roots can
    // live under the same pid dir, so `remove_dir_all` on it would delete theirs.
    if let Some(runtime_root) = handle
        .sock_path
        .parent()
        .and_then(|sock_dir| sock_dir.parent())
    {
        let _ = std::fs::remove_dir_all(runtime_root);
    }
}

/// Returns the new selection index after a navigation step within
/// `entry_count` entries. `up == true` moves toward 0; `false` moves toward
/// the last entry. Clamps at both ends.
fn step_selection(selected: usize, entry_count: usize, up: bool) -> usize {
    if up {
        selected.saturating_sub(1)
    } else if selected + 1 < entry_count {
        selected + 1
    } else {
        selected
    }
}

/// Logical pixels scrolled per wheel "line" notch. Roughly one row stride
/// (row height ≈ 18px + 2px gap).
const LINE_SCROLL_PX: f32 = 24.0;

/// The logical-pixel `ScrollPosition` delta for one wheel event. The sign is
/// inverted relative to the wheel `y` so that wheel-down (negative `y`) yields a
/// positive delta — a larger `ScrollPosition.0.y` moves the content up, i.e. the
/// viewport down. (Same structure as `tmux_inline_wheel_delta` but the opposite
/// sign, because that path drives a terminal, not a scroll offset.)
fn wheel_delta_px(unit: MouseScrollUnit, y: f32) -> f32 {
    match unit {
        MouseScrollUnit::Line => -y * LINE_SCROLL_PX,
        MouseScrollUnit::Pixel => -y,
    }
}

/// The new vertical scroll offset (logical px) that brings the row spanning
/// `[row_top, row_top + row_h]` fully into a `viewport_h`-tall viewport currently
/// scrolled to `current`. Scrolls up if the row is above the viewport, down if
/// below, else unchanged; the result is clamped to `[0, max]`.
fn reveal_offset(row_top: f32, row_h: f32, viewport_h: f32, current: f32, max: f32) -> f32 {
    let target = if row_top < current {
        row_top
    } else if row_top + row_h > current + viewport_h {
        row_top + row_h - viewport_h
    } else {
        current
    };
    target.clamp(0.0, max.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::AppExtStates;
    use bevy::state::app::StatesPlugin;
    use tmux_control::{SessionId, WindowId};

    fn fake_session(id: u32, name: &str) -> SessionInfo {
        SessionInfo {
            id: SessionId(id),
            name: name.to_string(),
            windows: 1,
            attached: false,
            created: 0,
        }
    }

    fn fake_window(session: u32, sname: &str, wid: u32, active: bool, wname: &str) -> WindowEntry {
        WindowEntry {
            session_id: SessionId(session),
            session_name: sname.to_string(),
            window_id: WindowId(wid),
            window_index: 0,
            window_active: active,
            window_name: wname.to_string(),
        }
    }

    #[test]
    fn build_rows_nests_windows_under_sessions_then_new_session() {
        let sessions = vec![fake_session(0, "alpha"), fake_session(1, "beta")];
        let windows = vec![
            fake_window(0, "alpha", 0, true, "zsh"),
            fake_window(0, "alpha", 1, false, "editor"),
            fake_window(1, "beta", 2, true, "top"),
        ];
        let rows = build_rows(&sessions, &windows);
        assert_eq!(
            rows,
            vec![
                PickerRow::Session(0),
                PickerRow::Window {
                    session: 0,
                    window: 0
                },
                PickerRow::Window {
                    session: 0,
                    window: 1
                },
                PickerRow::Session(1),
                PickerRow::Window {
                    session: 1,
                    window: 2
                },
                PickerRow::NewSession,
            ]
        );
    }

    #[test]
    fn build_rows_with_no_sessions_is_just_new_session() {
        assert_eq!(build_rows(&[], &[]), vec![PickerRow::NewSession]);
    }

    #[test]
    fn nav_arrow_down_increments() {
        assert_eq!(step_selection(0, 3, false), 1);
        assert_eq!(step_selection(1, 3, false), 2);
    }

    #[test]
    fn nav_arrow_down_clamps_at_last() {
        assert_eq!(step_selection(2, 3, false), 2);
    }

    #[test]
    fn nav_arrow_up_decrements() {
        assert_eq!(step_selection(2, 3, true), 1);
        assert_eq!(step_selection(1, 3, true), 0);
    }

    #[test]
    fn nav_arrow_up_clamps_at_zero() {
        assert_eq!(step_selection(0, 3, true), 0);
    }

    #[test]
    fn should_enter_ozmux_only_when_attached_from_ozma() {
        assert!(should_enter_ozmux(true, &AppMode::Ozma));
        assert!(!should_enter_ozmux(false, &AppMode::Ozma));
        assert!(!should_enter_ozmux(true, &AppMode::Ozmux));
        assert!(!should_enter_ozmux(false, &AppMode::Ozmux));
    }

    fn key_press(code: KeyCode) -> bevy::input::keyboard::KeyboardInput {
        bevy::input::keyboard::KeyboardInput {
            key_code: code,
            logical_key: bevy::input::keyboard::Key::Character("x".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    fn picker_input_app() -> App {
        let mut app = App::new();
        app.add_plugins(StatesPlugin);
        app.insert_state(AppMode::Ozma);
        app.add_message::<bevy::input::keyboard::KeyboardInput>();
        app.insert_resource(SessionPicker {
            sessions: vec![fake_session(0, "alpha"), fake_session(1, "beta")],
            windows: vec![fake_window(0, "alpha", 0, true, "zsh")],
            selected: 0,
            open: true,
            last_open: true,
        });
        app.init_resource::<ConnectionState>();
        app.init_resource::<OzmuxConfigsResource>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, handle_picker_input);
        app
    }

    fn send_key(app: &mut App, code: KeyCode) {
        app.world_mut().write_message(key_press(code));
        app.update();
    }

    #[test]
    fn j_k_move_selection_like_arrows() {
        let mut app = picker_input_app();
        send_key(&mut app, KeyCode::KeyJ);
        assert_eq!(app.world().resource::<SessionPicker>().selected, 1);
        send_key(&mut app, KeyCode::KeyK);
        assert_eq!(app.world().resource::<SessionPicker>().selected, 0);
    }

    #[test]
    fn esc_closes_the_picker() {
        let mut app = picker_input_app();
        assert!(app.world().resource::<SessionPicker>().open);
        send_key(&mut app, KeyCode::Escape);
        assert!(!app.world().resource::<SessionPicker>().open);
    }

    fn list_children(app: &mut App) -> Vec<Entity> {
        let mut q = app
            .world_mut()
            .query_filtered::<&Children, With<PickerList>>();
        match q.single(app.world()) {
            Ok(c) => (**c).to_vec(),
            Err(_) => Vec::new(),
        }
    }

    #[test]
    fn row_visuals_choose_tree_format() {
        let picker = SessionPicker {
            sessions: vec![
                fake_session(0, "alpha"),
                SessionInfo {
                    attached: true,
                    ..fake_session(1, "beta")
                },
            ],
            windows: vec![fake_window(0, "alpha", 0, true, "zsh")],
            selected: 0,
            open: true,
            last_open: true,
        };
        let rows = build_rows(&picker.sessions, &picker.windows);
        let v = row_visuals(&picker, &rows, 0);
        assert_eq!(v[0].0, "(0) alpha");
        assert!(!v[0].0.contains("windows"), "no window count");
        assert_eq!(v[1].0, "(1) └ 0: zsh*");
        assert_eq!(v[2].0, "(2) beta ·attached");
        assert_eq!(v[3].0, "(3) + New session");
        assert_eq!(v[0].2, theme::SELECTION);
        assert_eq!(v[0].1, theme::SELECTION_FG);
        assert_eq!(v[1].2, Color::NONE);
        assert_eq!(v[1].1, theme::MUTED);
        assert_eq!(v[2].1, theme::FOREGROUND);
    }

    fn dispatch_app() -> App {
        let mut app = App::new();
        app.add_plugins(StatesPlugin);
        app.insert_state(AppMode::Ozma);
        app.init_resource::<SessionPicker>();
        app.init_resource::<ConnectionState>();
        app.init_resource::<OzmuxConfigsResource>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(
            OnEnter(AppMode::Ozmux),
            dispatch_startup_mode.run_if(not(resource_exists::<StartupDispatched>)),
        );
        app
    }

    fn enter_ozmux(app: &mut App) {
        app.insert_resource(NextState::Pending(AppMode::Ozmux));
        app.update();
    }

    #[test]
    fn dispatch_runs_once_at_boot_and_marks_dispatched() {
        let mut app = dispatch_app();
        enter_ozmux(&mut app);
        // The boot dispatch ran: marker inserted, and (default startup_mode = Ozma)
        // it queued a transition back to Ozma.
        assert!(app.world().get_resource::<StartupDispatched>().is_some());
        app.update(); // apply the queued Ozmux -> Ozma transition
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Ozma
        );
    }

    #[test]
    fn dispatch_skipped_when_already_dispatched_does_not_bounce() {
        let mut app = dispatch_app();
        app.insert_resource(StartupDispatched);
        enter_ozmux(&mut app);
        // run_if(not(resource_exists)) is false, so dispatch never ran and the
        // Ozma -> Ozmux transition is NOT bounced back to Ozma.
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Ozmux
        );
    }

    fn cursor_moved() -> CursorMoved {
        CursorMoved {
            window: Entity::PLACEHOLDER,
            position: Vec2::ZERO,
            delta: None,
        }
    }

    #[test]
    fn hover_moves_selection_only_after_the_mouse_moves() {
        let mut app = App::new();
        app.add_plugins(StatesPlugin);
        app.insert_state(AppMode::Ozma);
        app.add_message::<CursorMoved>();
        app.insert_resource(SessionPicker {
            sessions: vec![fake_session(0, "alpha"), fake_session(1, "beta")],
            windows: vec![],
            selected: 0,
            open: true,
            last_open: true,
        });
        app.init_resource::<ConnectionState>();
        app.init_resource::<OzmuxConfigsResource>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, handle_picker_row_interaction);

        // A hovered row exists but the mouse has not moved since open: no change.
        let row = app.world_mut().spawn((Interaction::Hovered, PickerRowLabel(1))).id();
        app.update();
        assert_eq!(
            app.world().resource::<SessionPicker>().selected,
            0,
            "stationary-cursor hover must not move selection"
        );

        // Arm by moving the mouse, then re-trigger the hover change.
        app.world_mut().write_message(cursor_moved());
        app.world_mut()
            .entity_mut(row)
            .insert(Interaction::None);
        app.update();
        app.world_mut()
            .entity_mut(row)
            .insert(Interaction::Hovered);
        app.update();
        assert_eq!(
            app.world().resource::<SessionPicker>().selected,
            1,
            "after the mouse moves, hover moves selection"
        );
    }

    #[test]
    fn rows_spawn_as_buttons_carrying_their_index() {
        let mut app = App::new();
        app.insert_resource(SessionPicker {
            sessions: vec![fake_session(0, "alpha")],
            windows: vec![],
            selected: 0,
            open: true,
            last_open: true,
        });
        app.add_systems(Startup, spawn_picker_ui);
        app.add_systems(Update, sync_picker_ui);
        app.update();

        let mut q = app
            .world_mut()
            .query_filtered::<(&PickerRowLabel, Option<&Button>), With<PickerRowLabel>>();
        let mut indices: Vec<usize> = Vec::new();
        for (label, button) in q.iter(app.world()) {
            assert!(button.is_some(), "every picker row must carry Button");
            indices.push(label.0);
        }
        indices.sort_unstable();
        // build_rows([alpha], []) == [Session(0), NewSession] -> indices 0,1
        assert_eq!(indices, vec![0, 1]);
    }

    #[test]
    fn wheel_line_delta_is_inverted_and_scaled_by_row_stride() {
        // Wheel up (y>0) scrolls content toward the top -> negative offset delta.
        assert_eq!(wheel_delta_px(MouseScrollUnit::Line, 1.0), -LINE_SCROLL_PX);
        // Wheel down (y<0) -> positive offset delta.
        assert_eq!(wheel_delta_px(MouseScrollUnit::Line, -2.0), 2.0 * LINE_SCROLL_PX);
    }

    #[test]
    fn wheel_pixel_delta_is_inverted_identity() {
        assert_eq!(wheel_delta_px(MouseScrollUnit::Pixel, 5.0), -5.0);
        assert_eq!(wheel_delta_px(MouseScrollUnit::Pixel, -3.0), 3.0);
    }

    #[test]
    fn reveal_leaves_a_fully_visible_row_unchanged() {
        // row [40,60] inside viewport [30,130]: unchanged.
        assert_eq!(reveal_offset(40.0, 20.0, 100.0, 30.0, 200.0), 30.0);
    }

    #[test]
    fn reveal_scrolls_up_so_row_top_is_flush() {
        // row top 10 is above current 50: offset becomes 10.
        assert_eq!(reveal_offset(10.0, 20.0, 100.0, 50.0, 200.0), 10.0);
    }

    #[test]
    fn reveal_scrolls_down_so_row_bottom_is_flush() {
        // row [180,200], viewport height 100, current 0: 200 - 100 = 100.
        assert_eq!(reveal_offset(180.0, 20.0, 100.0, 0.0, 200.0), 100.0);
    }

    #[test]
    fn reveal_clamps_to_zero_and_to_max() {
        assert_eq!(reveal_offset(-10.0, 20.0, 100.0, 5.0, 200.0), 0.0);
        assert_eq!(reveal_offset(180.0, 20.0, 100.0, 0.0, 50.0), 50.0);
    }

    #[test]
    fn nav_reuses_row_entities_in_place() {
        let mut app = App::new();
        app.insert_resource(SessionPicker {
            sessions: vec![fake_session(0, "alpha"), fake_session(1, "beta")],
            windows: vec![fake_window(0, "alpha", 0, true, "zsh")],
            selected: 0,
            open: true,
            last_open: true,
        });
        app.add_systems(Startup, spawn_picker_ui);
        app.add_systems(Update, sync_picker_ui);
        app.update();

        let before = list_children(&mut app);
        assert!(!before.is_empty(), "rows should have been spawned");

        app.world_mut().resource_mut::<SessionPicker>().selected = 1;
        app.update();

        let after = list_children(&mut app);
        assert_eq!(
            before, after,
            "navigation must reuse row entities in place, not respawn them"
        );

        // The in-place update actually moved the highlight: exactly one row
        // carries the accent bar (the newly-selected one).
        let accent_rows = after
            .iter()
            .filter(|&&e| {
                app.world()
                    .get::<BackgroundColor>(e)
                    .is_some_and(|bg| bg.0 == theme::SELECTION)
            })
            .count();
        assert_eq!(accent_rows, 1, "exactly one row is highlighted after nav");
    }
}
