//! Renaming a workspace in its tab: the session, the text field, and the
//! rules that turn the field's text into a name.

use crate::font::TerminalUiFont;
use crate::input::bindings::OrzmaMouseConfig;
use crate::ui::tab_bar::{ACTIVE_TEXT, TabLabel, WorkspaceTab, tab_font, tab_label};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input_focus::{AutoFocus, FocusedInput, InputFocus};
use bevy::picking::PickingSettings;
use bevy::prelude::*;
use bevy::text::{EditableText, EditableTextFilter, EditableTextSystems, TextCursorStyle};
use bevy::ui_widgets::SelectAllOnFocus;
use bevy_cef::prelude::FocusedWebview;
use bevy_orzma_webview::RequestWebviewFocus;
use bevy_orzmux::prelude::{
    CurrentWorkspaces, OrzmuxSystems, RequestWorkspaceAction, WorkspaceAction, WorkspaceId,
};
use orzmux::prelude::MAX_WORKSPACE_NAME_CHARS;
use std::time::Duration;

/// The rename in progress, if any.
#[derive(Resource, Default, Debug)]
pub(crate) struct WorkspaceRename(Option<RenameSession>);

impl WorkspaceRename {
    /// Whether a rename is in progress, including one asked to end this
    /// frame.
    pub fn is_active(&self) -> bool {
        self.0.is_some()
    }

    /// The workspace being renamed.
    pub fn workspace(&self) -> Option<WorkspaceId> {
        self.0.as_ref().map(|session| session.workspace)
    }

    fn field(&self) -> Option<Entity> {
        self.0.as_ref().map(|session| session.field)
    }

    fn is_ending(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(|session| session.ending.is_some())
    }

    fn can_end(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(|session| session.ending.is_none())
    }

    fn request_end(&mut self, commit: bool) {
        if let Some(session) = self.0.as_mut() {
            session.ending = Some(commit);
        }
    }
}

#[cfg(test)]
impl WorkspaceRename {
    /// A session renaming `workspace` that is still editing.
    pub fn active_for_test(workspace: WorkspaceId) -> Self {
        Self(Some(RenameSession {
            workspace,
            field: Entity::PLACEHOLDER,
            label: Entity::PLACEHOLDER,
            name: None,
            auto_label: "Workspace 1".into(),
            ending: None,
        }))
    }

    /// A session renaming `workspace` through `field` in place of `label`,
    /// already asked to end with `commit`.
    pub fn ending_for_test(
        workspace: WorkspaceId,
        field: Entity,
        label: Entity,
        commit: bool,
    ) -> Self {
        Self(Some(RenameSession {
            workspace,
            field,
            label,
            name: None,
            auto_label: "Workspace 1".into(),
            ending: Some(commit),
        }))
    }
}

/// The `PostUpdate` slot where an ending rename is committed or discarded,
/// after the field's edits of the frame are applied.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RenameSystems {
    /// Committing or discarding the ending rename.
    Finish,
}

/// Starts renaming a workspace in its tab, unless a rename is already in
/// progress.
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct StartWorkspaceRename {
    /// The workspace to rename; `None` names the displayed one.
    pub workspace: Option<WorkspaceId>,
}

/// The rename ending a key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenameKey {
    /// Keep the field's text.
    Commit,
    /// Keep the old name.
    Cancel,
}

impl RenameKey {
    /// The ending a pressed Enter or Esc asks for, or `None` for a release,
    /// any other key, or a key pressed during an IME composition.
    pub fn classify(input: &KeyboardInput, composing: bool) -> Option<Self> {
        if composing || input.state != ButtonState::Pressed {
            return None;
        }
        match input.logical_key {
            Key::Enter => Some(Self::Commit),
            Key::Escape => Some(Self::Cancel),
            _ => None,
        }
    }
}

/// What committing the field's text does to the workspace's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RenameOutcome {
    /// The name stays as it is.
    Keep,
    /// The name becomes this; `None` restores the automatic name.
    Set(Option<String>),
}

impl RenameOutcome {
    /// Decides the outcome of committing `input` for a workspace named
    /// `name` (`None` when automatic) whose automatic label is
    /// `auto_label`. Surrounding whitespace in `input` is ignored.
    pub fn decide(input: &str, name: Option<&str>, auto_label: &str) -> Self {
        let input = input.trim();
        if input.is_empty() {
            return if name.is_none() {
                Self::Keep
            } else {
                Self::Set(None)
            };
        }
        let unchanged = match name {
            Some(name) => input == name,
            None => input == auto_label,
        };
        if unchanged {
            Self::Keep
        } else {
            Self::Set(Some(input.to_string()))
        }
    }
}

/// Lets a workspace be renamed in its tab, and gives a tab double-click the
/// terminal's double-click interval.
pub(crate) struct TabRenamePlugin;

impl Plugin for TabRenamePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkspaceRename>()
            .configure_sets(PostUpdate, RenameSystems::Finish.after(EditableTextSystems))
            .add_observer(start_rename)
            .add_observer(commit_on_outside_press)
            .add_systems(
                Update,
                (
                    drop_rename_of_closed_workspace
                        .after(OrzmuxSystems::Drain)
                        .run_if(resource_exists_and_changed::<CurrentWorkspaces>),
                    commit_on_webview_focus
                        .after(OrzmuxSystems::Drain)
                        .run_if(resource_exists_and_changed::<FocusedWebview>),
                    align_multi_click_interval
                        .run_if(resource_exists_and_changed::<OrzmaMouseConfig>),
                ),
            )
            .add_systems(
                PostUpdate,
                finish_rename
                    .in_set(RenameSystems::Finish)
                    .run_if(rename_is_ending),
            );
    }
}

/// A blink period long enough that the caret stays drawn.
const STEADY_CARET: Duration = Duration::from_secs(3600);
/// The background of the rename field's selected text.
const SELECTION_BG: Color = Color::srgb_u8(0x3f, 0x63, 0x8b);

/// One rename: the workspace, its field, the label the field replaces, the
/// name and automatic label the workspace had, and how the rename ends.
#[derive(Debug, Clone)]
struct RenameSession {
    workspace: WorkspaceId,
    field: Entity,
    label: Entity,
    name: Option<String>,
    auto_label: String,
    /// `Some(true)` to commit and `Some(false)` to discard at the next
    /// [`RenameSystems::Finish`]; `None` while editing.
    ending: Option<bool>,
}

/// Replaces the tab's label with a focused text field that holds the label,
/// all of it selected, and draws a caret in the label's text color and a
/// highlight behind the selection; releases webview focus.
fn start_rename(
    ev: On<StartWorkspaceRename>,
    mut commands: Commands,
    mut rename: ResMut<WorkspaceRename>,
    mut labels: Query<&mut Node, With<TabLabel>>,
    workspaces: Res<CurrentWorkspaces>,
    tabs: Query<(Entity, &WorkspaceTab, &Children)>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    if rename.is_active() {
        return;
    }
    let Some(workspace) = ev.workspace.or(workspaces.active) else {
        return;
    };
    let Some(position) = workspaces.position_of(workspace) else {
        return;
    };
    let name = workspaces
        .entries
        .get(position)
        .and_then(|entry| entry.name.clone());
    let Some((tab, _, parts)) = tabs.iter().find(|(_, tab, _)| tab.workspace == workspace) else {
        return;
    };
    let Some((slot, label)) = parts
        .iter()
        .enumerate()
        .find(|(_, part)| labels.contains(*part))
    else {
        return;
    };
    if let Ok(mut node) = labels.get_mut(label)
        && node.display != Display::None
    {
        node.display = Display::None;
    }
    let field = commands
        .spawn((
            EditableText {
                max_characters: Some(MAX_WORKSPACE_NAME_CHARS),
                allow_newlines: false,
                cursor_blink_period: STEADY_CARET,
                ..EditableText::new(tab_label(position, name.as_deref()))
            },
            EditableTextFilter::new(|c| !c.is_control()),
            SelectAllOnFocus,
            AutoFocus,
            tab_font(ui_font.as_deref()),
            TextColor(ACTIVE_TEXT),
            TextLayout::no_wrap(),
            TextCursorStyle {
                color: ACTIVE_TEXT,
                selection_color: SELECTION_BG,
                unfocused_selection_color: Color::NONE,
                selected_text_color: None,
            },
            Node {
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                ..default()
            },
        ))
        .observe(on_field_key)
        .id();
    commands.entity(tab).insert_children(slot, &[field]);
    commands.trigger(RequestWebviewFocus::new(None));
    rename.0 = Some(RenameSession {
        workspace,
        field,
        label,
        name,
        auto_label: tab_label(position, None),
        ending: None,
    });
}

/// Asks to commit on Enter and to cancel on Esc, outside IME compositions.
fn on_field_key(
    ev: On<FocusedInput<KeyboardInput>>,
    mut rename: ResMut<WorkspaceRename>,
    fields: Query<&EditableText>,
) {
    let composing = fields
        .get(ev.focused_entity)
        .is_ok_and(EditableText::is_composing);
    let Some(key) = RenameKey::classify(&ev.input, composing) else {
        return;
    };
    if rename.can_end() {
        rename.request_end(key == RenameKey::Commit);
    }
}

/// Asks to commit when a press lands anywhere but the field.
fn commit_on_outside_press(ev: On<Pointer<Press>>, mut rename: ResMut<WorkspaceRename>) {
    let origin = ev.original_event_target();
    if ev.entity != origin || rename.field() == Some(origin) || !rename.can_end() {
        return;
    }
    rename.request_end(true);
}

/// Asks to commit when a webview takes the keyboard focus.
fn commit_on_webview_focus(mut rename: ResMut<WorkspaceRename>, focused: Res<FocusedWebview>) {
    if focused.0.is_some() && rename.can_end() {
        rename.request_end(true);
    }
}

/// Commits or discards the ending rename, restores the label, removes the
/// field, and takes the keyboard focus off it.
fn finish_rename(
    mut commands: Commands,
    mut rename: ResMut<WorkspaceRename>,
    mut input_focus: ResMut<InputFocus>,
    mut labels: Query<&mut Node, With<TabLabel>>,
    fields: Query<&EditableText>,
) {
    let Some(session) = rename.0.take() else {
        return;
    };
    if session.ending == Some(true)
        && let Ok(field) = fields.get(session.field)
        && let RenameOutcome::Set(name) = RenameOutcome::decide(
            &field.value().to_string(),
            session.name.as_deref(),
            &session.auto_label,
        )
    {
        commands.trigger(RequestWorkspaceAction {
            action: WorkspaceAction::Rename {
                workspace: session.workspace,
                name,
            },
        });
    }
    if let Ok(mut node) = labels.get_mut(session.label)
        && node.display == Display::None
    {
        node.display = Display::Flex;
    }
    commands.entity(session.field).try_despawn();
    if input_focus.get() == Some(session.field) {
        input_focus.clear();
    }
}

/// Whether a rename waits to be finished.
fn rename_is_ending(rename: Res<WorkspaceRename>) -> bool {
    rename.is_ending()
}

/// Drops, without a request, a rename whose workspace closed, and takes
/// the keyboard focus off its field.
fn drop_rename_of_closed_workspace(
    mut rename: ResMut<WorkspaceRename>,
    mut input_focus: ResMut<InputFocus>,
    workspaces: Res<CurrentWorkspaces>,
) {
    let Some(workspace) = rename.workspace() else {
        return;
    };
    if workspaces.position_of(workspace).is_some() {
        return;
    }
    let field = rename.field();
    rename.0 = None;
    if field.is_some() && input_focus.get() == field {
        input_focus.clear();
    }
}

/// Makes a tab double-click use the terminal's double-click interval.
fn align_multi_click_interval(mut settings: ResMut<PickingSettings>, mouse: Res<OrzmaMouseConfig>) {
    if settings.multi_click_interval != mouse.double_click_timeout {
        settings.multi_click_interval = mouse.double_click_timeout;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::{Location, PointerId};
    use bevy_orzmux::prelude::WorkspaceEntry;

    /// Asserts the commit rules: blank restores the automatic name, the
    /// untouched automatic label or the same name keeps things as they
    /// are, and anything else sets the name.
    ///
    /// Case: the user opens the rename field on tabs with and without a
    /// name and confirms various inputs.
    #[test]
    fn the_commit_rules_decide_the_name() {
        assert_eq!(
            RenameOutcome::decide("  ", Some("logs"), "Workspace 2"),
            RenameOutcome::Set(None)
        );
        assert_eq!(
            RenameOutcome::decide("", None, "Workspace 2"),
            RenameOutcome::Keep
        );
        assert_eq!(
            RenameOutcome::decide("Workspace 2", None, "Workspace 2"),
            RenameOutcome::Keep
        );
        assert_eq!(
            RenameOutcome::decide("logs", Some("logs"), "Workspace 2"),
            RenameOutcome::Keep
        );
        assert_eq!(
            RenameOutcome::decide(" build ", None, "Workspace 2"),
            RenameOutcome::Set(Some("build".into()))
        );
    }

    fn key(logical_key: Key, state: ButtonState) -> KeyboardInput {
        KeyboardInput {
            key_code: KeyCode::Enter,
            logical_key,
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    /// Asserts that a pressed Enter commits and Esc cancels, while a
    /// release, another key, or any key during an IME composition ends
    /// nothing.
    ///
    /// Case: the user converts kana with Enter while renaming, then
    /// confirms the name with Enter.
    #[test]
    fn only_a_pressed_enter_or_escape_outside_a_composition_ends_the_rename() {
        assert_eq!(
            RenameKey::classify(&key(Key::Enter, ButtonState::Pressed), false),
            Some(RenameKey::Commit)
        );
        assert_eq!(
            RenameKey::classify(&key(Key::Escape, ButtonState::Pressed), false),
            Some(RenameKey::Cancel)
        );
        assert_eq!(
            RenameKey::classify(&key(Key::Enter, ButtonState::Pressed), true),
            None
        );
        assert_eq!(
            RenameKey::classify(&key(Key::Escape, ButtonState::Pressed), true),
            None
        );
        assert_eq!(
            RenameKey::classify(&key(Key::Enter, ButtonState::Released), false),
            None
        );
        assert_eq!(
            RenameKey::classify(&key(Key::Tab, ButtonState::Pressed), false),
            None
        );
    }

    #[derive(Resource, Default)]
    struct Requested(Vec<WorkspaceAction>);

    fn finish_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputFocus>()
            .init_resource::<Requested>()
            .add_observer(
                |ev: On<RequestWorkspaceAction>, mut seen: ResMut<Requested>| {
                    seen.0.push(ev.action.clone());
                },
            )
            .add_systems(PostUpdate, finish_rename.run_if(rename_is_ending));
        app
    }

    fn press_on(entity: Entity) -> Pointer<Press> {
        let event = Press {
            button: PointerButton::Primary,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            count: 1,
        };
        let location = Location {
            target: NormalizedRenderTarget::None {
                width: 800,
                height: 600,
            },
            position: Vec2::ZERO,
        };
        Pointer::new(PointerId::Mouse, location, event, entity)
    }

    /// Asserts that finishing a committed rename sends the field's name,
    /// restores the label, despawns the field, and ends the session.
    ///
    /// Case: the user types a name and presses Enter.
    #[test]
    fn finishing_a_commit_sends_the_name_and_ends_the_session() {
        let mut app = finish_app();
        let label = app
            .world_mut()
            .spawn((
                TabLabel,
                Node {
                    display: Display::None,
                    ..default()
                },
            ))
            .id();
        let field = app.world_mut().spawn(EditableText::new("logs")).id();
        app.insert_resource(WorkspaceRename::ending_for_test(
            WorkspaceId(1),
            field,
            label,
            true,
        ));
        app.update();
        assert_eq!(
            app.world().resource::<Requested>().0,
            vec![WorkspaceAction::Rename {
                workspace: WorkspaceId(1),
                name: Some("logs".into())
            }]
        );
        assert!(!app.world().resource::<WorkspaceRename>().is_active());
        assert!(app.world().get_entity(field).is_err());
        assert_eq!(
            app.world().get::<Node>(label).map(|n| n.display),
            Some(Display::Flex)
        );
    }

    /// Asserts that finishing a cancelled rename sends nothing and still
    /// ends the session.
    ///
    /// Case: the user presses Esc in the rename field.
    #[test]
    fn finishing_a_cancel_sends_nothing() {
        let mut app = finish_app();
        let label = app
            .world_mut()
            .spawn((
                TabLabel,
                Node {
                    display: Display::None,
                    ..default()
                },
            ))
            .id();
        let field = app.world_mut().spawn(EditableText::new("logs")).id();
        app.insert_resource(WorkspaceRename::ending_for_test(
            WorkspaceId(1),
            field,
            label,
            false,
        ));
        app.update();
        assert!(app.world().resource::<Requested>().0.is_empty());
        assert!(!app.world().resource::<WorkspaceRename>().is_active());
    }

    /// Asserts that a press whose original target is not the rename field
    /// commits the rename, while a press on the field, even one that
    /// bubbles up to its tab, leaves the rename editing.
    ///
    /// Case: the user clicks inside the field to move the caret, then
    /// clicks a pane to leave the field.
    #[test]
    fn a_press_outside_the_field_commits_and_one_inside_does_not() {
        let mut app = finish_app();
        app.add_observer(commit_on_outside_press);
        app.world_mut().register_component::<Window>();
        let tab = app.world_mut().spawn_empty().id();
        let field = app
            .world_mut()
            .spawn((EditableText::new("build"), ChildOf(tab)))
            .id();
        let pane = app.world_mut().spawn_empty().id();
        app.insert_resource(WorkspaceRename(Some(RenameSession {
            workspace: WorkspaceId(1),
            field,
            label: Entity::PLACEHOLDER,
            name: None,
            auto_label: "Workspace 1".into(),
            ending: None,
        })));

        app.world_mut().trigger(press_on(field));
        app.update();

        assert!(app.world().resource::<WorkspaceRename>().is_active());
        assert!(app.world().resource::<Requested>().0.is_empty());

        app.world_mut().trigger(press_on(pane));
        app.update();

        assert!(!app.world().resource::<WorkspaceRename>().is_active());
        assert_eq!(
            app.world().resource::<Requested>().0,
            vec![WorkspaceAction::Rename {
                workspace: WorkspaceId(1),
                name: Some("build".into())
            }]
        );
    }

    /// Asserts that a rename of a workspace that closes meanwhile is
    /// dropped without a request.
    ///
    /// Case: the renamed tab's last shell exits while the user types.
    #[test]
    fn a_rename_of_a_closed_workspace_is_dropped() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputFocus>()
            .init_resource::<CurrentWorkspaces>()
            .insert_resource(WorkspaceRename::active_for_test(WorkspaceId(1)))
            .add_systems(
                Update,
                drop_rename_of_closed_workspace
                    .run_if(resource_exists_and_changed::<CurrentWorkspaces>),
            );
        app.world_mut().resource_mut::<CurrentWorkspaces>().entries = vec![WorkspaceEntry {
            id: WorkspaceId(2),
            name: None,
        }];
        app.update();
        assert!(!app.world().resource::<WorkspaceRename>().is_active());
    }

    /// Asserts that a webview taking the keyboard focus commits the
    /// rename, while the webview focus release that starting the rename
    /// makes leaves it editing.
    ///
    /// Case: the user starts renaming a tab while a page holds the keyboard
    /// focus, types a name, and a program in another pane then focuses its
    /// page through the control socket.
    #[test]
    fn a_webview_taking_focus_commits_the_rename() {
        let mut app = finish_app();
        app.init_resource::<WorkspaceRename>()
            .init_resource::<CurrentWorkspaces>()
            .init_resource::<FocusedWebview>()
            .add_observer(start_rename)
            .add_observer(
                |ev: On<RequestWebviewFocus>, mut focused: ResMut<FocusedWebview>| {
                    focused.0 = ev.target();
                },
            )
            .add_systems(
                Update,
                commit_on_webview_focus.run_if(resource_exists_and_changed::<FocusedWebview>),
            );
        {
            let mut workspaces = app.world_mut().resource_mut::<CurrentWorkspaces>();
            workspaces.entries = vec![WorkspaceEntry {
                id: WorkspaceId(1),
                name: None,
            }];
            workspaces.active = Some(WorkspaceId(1));
        }
        let tab = app
            .world_mut()
            .spawn(WorkspaceTab {
                workspace: WorkspaceId(1),
            })
            .id();
        app.world_mut()
            .spawn((TabLabel, Node::default(), ChildOf(tab)));
        let page = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(page);
        app.update();

        app.world_mut()
            .trigger(StartWorkspaceRename { workspace: None });
        app.update();

        assert_eq!(app.world().resource::<FocusedWebview>().0, None);
        assert!(app.world().resource::<WorkspaceRename>().can_end());
        let field = app
            .world()
            .resource::<WorkspaceRename>()
            .field()
            .expect("the rename has a field");
        app.world_mut()
            .entity_mut(field)
            .insert(EditableText::new("build"));

        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(page);
        app.update();

        assert!(!app.world().resource::<WorkspaceRename>().is_active());
        assert_eq!(
            app.world().resource::<Requested>().0,
            vec![WorkspaceAction::Rename {
                workspace: WorkspaceId(1),
                name: Some("build".into())
            }]
        );
    }
}
