//! Startup tmux session picker: lists sessions, shows a keyboard-navigable
//! overlay, and attaches only after the user selects an entry (or "New session").

use crate::configs::OzmuxConfigsResource;
use crate::control_plane::ControlPlaneHandle;
use crate::theme;
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

#[derive(Component)]
struct PickerRowLabel;

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
            parent
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Stretch,
                        min_width: Val::Px(360.0),
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
                            ..default()
                        },
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
            for (label, text_color, bar_color) in visuals {
                parent.spawn((
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
                    PickerRowLabel,
                ));
            }
        });
    }
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
    if connection.client().is_none() {
        return;
    }
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
