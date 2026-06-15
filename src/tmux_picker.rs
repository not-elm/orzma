//! Startup tmux session picker: lists sessions, shows a keyboard-navigable
//! overlay, and attaches only after the user selects an entry (or "New session").

use crate::configs::OzmuxConfigsResource;
use bevy::ecs::hierarchy::Children;
use bevy::ecs::message::MessageReader;
use bevy::ecs::schedule::common_conditions::resource_exists_and_changed;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use ozmux_tmux::{ConnectionState, TmuxConnection, attach_or_create};
use tmux_control::{SessionInfo, TmuxServer};

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
            .add_systems(
                PostUpdate,
                sync_picker_ui.run_if(resource_exists_and_changed::<SessionPicker>),
            );
    }
}

#[derive(Resource, Default)]
pub(crate) struct SessionPicker {
    sessions: Vec<SessionInfo>,
    selected: usize,
    pub(crate) open: bool,
}

#[derive(Component)]
struct PickerBackdrop;

#[derive(Component)]
struct PickerList;

fn target_for(sessions: &[SessionInfo], selected: usize) -> ozmux_tmux::AttachTarget {
    match sessions.get(selected) {
        Some(s) => ozmux_tmux::AttachTarget::Attach(s.name.clone()),
        None => ozmux_tmux::AttachTarget::CreateNew,
    }
}

fn build_server(configs: &OzmuxConfigsResource) -> TmuxServer {
    let cfg = &configs.tmux;
    let mut server = TmuxServer::new().program(&cfg.program);
    if let Some(name) = &cfg.socket_name {
        server = server.socket_name(name);
    }
    server
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

    let entry_count = picker.sessions.len() + 1;
    let mut child_commands = commands.entity(list_entity);
    child_commands.with_children(|parent| {
        for i in 0..entry_count {
            let is_selected = i == picker.selected;
            let prefix = if is_selected { "> " } else { "  " };
            let label = if i < picker.sessions.len() {
                let s = &picker.sessions[i];
                let attached_suffix = if s.attached { " *attached" } else { "" };
                format!(
                    "{}{}  ({} windows){}",
                    prefix, s.name, s.windows, attached_suffix
                )
            } else {
                format!("{}+ New session", prefix)
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
) {
    if !picker.open {
        keys.clear();
        return;
    }
    let entry_count = picker.sessions.len() + 1;
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
                let target = target_for(&picker.sessions, picker.selected);
                let server = build_server(&configs);
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
                picker.open = false;
                break;
            }
            _ => {}
        }
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
    use tmux_control::SessionId;

    fn fake_session(id: u32, name: &str) -> SessionInfo {
        SessionInfo {
            id: SessionId(id),
            name: name.to_string(),
            windows: 1,
            attached: false,
            created: 0,
        }
    }

    #[test]
    fn target_for_empty_sessions_gives_create_new() {
        assert_eq!(target_for(&[], 0), ozmux_tmux::AttachTarget::CreateNew,);
    }

    #[test]
    fn target_for_session_index_gives_attach() {
        let sessions = vec![fake_session(1, "a"), fake_session(2, "b")];
        assert_eq!(
            target_for(&sessions, 0),
            ozmux_tmux::AttachTarget::Attach("a".to_string()),
        );
        assert_eq!(
            target_for(&sessions, 1),
            ozmux_tmux::AttachTarget::Attach("b".to_string()),
        );
    }

    #[test]
    fn target_for_trailing_entry_gives_create_new() {
        let sessions = vec![fake_session(1, "a"), fake_session(2, "b")];
        assert_eq!(
            target_for(&sessions, 2),
            ozmux_tmux::AttachTarget::CreateNew,
        );
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
