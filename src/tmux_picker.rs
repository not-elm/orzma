//! Startup tmux session picker: lists sessions, shows a keyboard-navigable
//! overlay, and attaches only after the user selects an entry (or "New session").

use crate::configs::OzmuxConfigsResource;
use crate::control_plane::ControlPlaneHandle;
use bevy::ecs::hierarchy::Children;
use bevy::ecs::message::MessageReader;
use bevy::ecs::schedule::common_conditions::resource_exists_and_changed;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use ozmux_tmux::{
    AttachTarget, ConnectionState, TmuxConnection, attach_or_create, select_window_command,
    set_environment_command, switch_client_command,
};
use tmux_control::{SessionInfo, TmuxServer, WindowEntry};

const PICKER_Z: i32 = 310;

/// Registers the session picker UI and keyboard handler.
pub(crate) struct OzmuxTmuxPickerPlugin;

impl Plugin for OzmuxTmuxPickerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SessionPicker>()
            .add_systems(Startup, (list_sessions_into_picker, spawn_picker_ui))
            .add_systems(
                Update,
                handle_picker_input.after(crate::input::InputPhase::FocusedKey),
            )
            .add_systems(Update, refresh_picker_on_open)
            .add_systems(
                Update,
                inject_session_ozmux_sock.run_if(resource_exists_and_changed::<ConnectionState>),
            )
            .add_systems(
                PostUpdate,
                sync_picker_ui.run_if(resource_exists_and_changed::<SessionPicker>),
            );
    }
}

/// One selectable row in the chooser tree: a session header, a window under a
/// session (indices into the picker's `sessions` / `windows`), or the trailing
/// "New session" entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerRow {
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

#[derive(Component)]
struct PickerBackdrop;

#[derive(Component)]
struct PickerList;

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

fn list_sessions_into_picker(
    mut picker: ResMut<SessionPicker>,
    mut state: ResMut<ConnectionState>,
    configs: Res<OzmuxConfigsResource>,
) {
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
            parent.spawn(Text::new("Select tmux session"));
            parent.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::FlexStart,
                    ..default()
                },
                PickerList,
            ));
        });
}

fn sync_picker_ui(
    mut backdrop: Query<&mut Node, With<PickerBackdrop>>,
    mut commands: Commands,
    picker: Res<SessionPicker>,
    list_query: Query<Entity, With<PickerList>>,
) {
    let Ok(mut node) = backdrop.single_mut() else {
        return;
    };
    node.display = if picker.open {
        Display::Flex
    } else {
        Display::None
    };

    let Ok(list_entity) = list_query.single() else {
        return;
    };

    commands.entity(list_entity).despawn_related::<Children>();

    let rows = build_rows(&picker.sessions, &picker.windows);
    let selected = picker.selected.min(rows.len().saturating_sub(1));
    let mut child_commands = commands.entity(list_entity);
    child_commands.with_children(|parent| {
        for (i, row) in rows.iter().enumerate() {
            let is_selected = i == selected;
            let prefix = if is_selected { "> " } else { "  " };
            let label = match row {
                PickerRow::Session(si) => {
                    let s = &picker.sessions[*si];
                    let attached = if s.attached { " *attached" } else { "" };
                    format!("{}{}  ({} windows){}", prefix, s.name, s.windows, attached)
                }
                PickerRow::Window { window, .. } => {
                    let w = &picker.windows[*window];
                    let active = if w.window_active { "*" } else { " " };
                    format!(
                        "{}    {}{}: {}",
                        prefix, active, w.window_index, w.window_name
                    )
                }
                PickerRow::NewSession => format!("{}+ New session", prefix),
            };
            let color = if is_selected {
                TextColor(Color::WHITE)
            } else {
                TextColor(Color::srgba(0.6, 0.6, 0.6, 1.0))
            };
            parent.spawn((Text::new(label), color));
        }
    });
}

fn handle_picker_input(
    mut picker: ResMut<SessionPicker>,
    mut connection: NonSendMut<TmuxConnection>,
    mut state: ResMut<ConnectionState>,
    mut keys: MessageReader<KeyboardInput>,
    configs: Res<OzmuxConfigsResource>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    if !picker.open {
        keys.clear();
        return;
    }
    let entry_count = build_rows(&picker.sessions, &picker.windows).len();
    for ev in keys.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match ev.key_code {
            KeyCode::ArrowUp => {
                picker.selected = step_selection(picker.selected, entry_count, true);
            }
            KeyCode::ArrowDown => {
                picker.selected = step_selection(picker.selected, entry_count, false);
            }
            KeyCode::Enter => {
                let rows = build_rows(&picker.sessions, &picker.windows);
                let row = rows
                    .get(picker.selected)
                    .copied()
                    .unwrap_or(PickerRow::NewSession);
                if connection.client().is_some() {
                    apply_switch(&mut connection, &mut state, &configs, &picker, row);
                } else {
                    apply_attach(
                        &mut connection,
                        &mut state,
                        &configs,
                        control.as_deref(),
                        &picker,
                        row,
                    );
                }
                picker.open = false;
                break;
            }
            _ => {}
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
    picker: &SessionPicker,
    row: PickerRow,
) {
    let Some(client) = connection.client() else {
        return;
    };
    let cmds: Vec<String> = match row {
        PickerRow::Session(si) => vec![switch_client_command(&picker.sessions[si].name)],
        PickerRow::Window { session, window } => vec![
            switch_client_command(&picker.sessions[session].name),
            select_window_command(picker.windows[window].window_id),
        ],
        PickerRow::NewSession => {
            let server = build_server(configs);
            match server.create_detached_session() {
                Ok(name) => vec![switch_client_command(&name)],
                Err(e) => {
                    tracing::warn!(?e, "failed to create new session");
                    return;
                }
            }
        }
    };
    for cmd in &cmds {
        if let Err(e) = client.handle().send(cmd) {
            tracing::warn!(?e, cmd, "switch command send failed");
            *state = ConnectionState::Error {
                reason: format!("switch failed: {e}"),
            };
            return;
        }
    }
}

fn apply_attach(
    connection: &mut TmuxConnection,
    state: &mut ConnectionState,
    configs: &OzmuxConfigsResource,
    control: Option<&ControlPlaneHandle>,
    picker: &SessionPicker,
    row: PickerRow,
) {
    let target = match row {
        PickerRow::Session(si) => AttachTarget::Attach(picker.sessions[si].name.clone()),
        PickerRow::Window { session, .. } => {
            AttachTarget::Attach(picker.sessions[session].name.clone())
        }
        PickerRow::NewSession => AttachTarget::CreateNew,
    };
    let mut server = build_server(configs);
    if let Some(handle) = control {
        server = server.env("OZMUX_SOCK", &handle.sock_path.to_string_lossy());
    }
    match attach_or_create(&server, &target) {
        Ok(client) => {
            connection.set(client);
            *state = ConnectionState::Connecting;
        }
        Err(e) => {
            *state = ConnectionState::Error {
                reason: format!("tmux connect failed: {e}"),
            };
        }
    }
}

/// Sets `$OZMUX_SOCK` in the freshly-connected session's tmux environment so
/// panes created after attach inherit it. Gated to run on every
/// [`ConnectionState`] change; the body acts only on the `Attached` transition.
///
/// `new-session` already injects `$OZMUX_SOCK` via `-e` (covering its initial
/// pane), but attaching to a pre-existing session cannot — its panes are spawned
/// by an already-running server. This session-scoped `set-environment` closes
/// that gap for panes opened after attach (already-running shells are
/// unreachable). No-op when the control plane is down.
fn inject_session_ozmux_sock(
    state: Res<ConnectionState>,
    connection: NonSend<TmuxConnection>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    if !matches!(*state, ConnectionState::Attached) {
        return;
    }
    let (Some(handle), Some(client)) = (control, connection.client()) else {
        return;
    };
    let cmd = set_environment_command("OZMUX_SOCK", &handle.sock_path.to_string_lossy());
    if let Err(e) = client.handle().send(&cmd) {
        tracing::warn!(?e, "failed to set $OZMUX_SOCK in tmux session environment");
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
