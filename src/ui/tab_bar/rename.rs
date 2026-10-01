//! Renaming a tab in place: the session, the text field, and the
//! rules that turn the field's text into a name.

use crate::font::TerminalUiFont;
use crate::input::bindings::OrzmaMouseConfig;
use crate::ui::tab_bar::{ACTIVE_TEXT, TabButton, TabLabel, TabLabelText, tab_font, tab_label};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input_focus::{AutoFocus, FocusedInput, InputFocus};
use bevy::picking::PickingSettings;
use bevy::prelude::*;
use bevy::text::{EditableText, EditableTextFilter, EditableTextSystems, TextCursorStyle};
use bevy::ui::widget::measure_text_system;
use bevy::ui_widgets::SelectAllOnFocus;
use bevy_cef::prelude::FocusedWebview;
use bevy_orzma_webview::RequestWebviewFocus;
use bevy_orzmux::prelude::{CurrentTabs, OrzmuxSystems, RequestTabAction, TabAction, TabId};
use orzmux::prelude::Tab;
use std::time::Duration;

/// The rename in progress, if any.
#[derive(Resource, Default, Debug)]
pub(crate) struct TabRename(Option<RenameSession>);

impl TabRename {
    /// Whether a rename is in progress, including one asked to end this
    /// frame.
    pub fn is_active(&self) -> bool {
        self.0.is_some()
    }

    /// The tab being renamed.
    pub fn tab(&self) -> Option<TabId> {
        self.0.as_ref().map(|session| session.tab)
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
impl TabRename {
    /// A session renaming `tab` that is still editing.
    pub fn active_for_test(tab: TabId) -> Self {
        Self(Some(RenameSession {
            tab,
            field: Entity::PLACEHOLDER,
            label: Entity::PLACEHOLDER,
            name: None,
            initial: "Tab 1".into(),
            ending: None,
        }))
    }

    /// A session renaming `tab` through `field` in place of `label`,
    /// already asked to end with `commit`.
    pub fn ending_for_test(tab: TabId, field: Entity, label: Entity, commit: bool) -> Self {
        Self(Some(RenameSession {
            tab,
            field,
            label,
            name: None,
            initial: "Tab 1".into(),
            ending: Some(commit),
        }))
    }
}

/// The `PostUpdate` slot where an ending rename is committed or discarded,
/// after the field's edits of the frame are applied and before the UI
/// measures text, so the tab shows the committed name in the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RenameSystems {
    /// Committing or discarding the ending rename.
    Finish,
}

/// Starts renaming a tab in place, unless a rename is already in
/// progress.
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct StartTabRename {
    /// The tab to rename; `None` names the displayed one.
    pub tab: Option<TabId>,
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

/// What committing the field's text does to the tab's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RenameOutcome {
    /// The name stays as it is.
    Keep,
    /// The name becomes this; `None` restores the automatic name.
    Set(Option<String>),
}

impl RenameOutcome {
    /// Decides the outcome of committing `input` for a tab named `name`
    /// (`None` when unnamed) whose field started as `initial`. Surrounding
    /// whitespace in `input` is ignored.
    pub fn decide(input: &str, name: Option<&str>, initial: &str) -> Self {
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
            None => input == initial,
        };
        if unchanged {
            Self::Keep
        } else {
            Self::Set(Some(input.to_string()))
        }
    }
}

/// Lets a tab be renamed in place, and gives a tab double-click the
/// terminal's double-click interval.
pub(crate) struct TabRenamePlugin;

impl Plugin for TabRenamePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TabRename>()
            .configure_sets(
                PostUpdate,
                RenameSystems::Finish
                    .after(EditableTextSystems)
                    .before(measure_text_system),
            )
            .add_observer(start_rename)
            .add_observer(commit_on_outside_press)
            .add_systems(
                Update,
                (
                    drop_rename_of_closed_tab
                        .after(OrzmuxSystems::Drain)
                        .run_if(resource_exists_and_changed::<CurrentTabs>),
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
const SELECTION_BG: Color = Color::srgb_u8(0x4c, 0x1d, 0x95);

/// One rename: the tab, its field, the label the field replaces, the
/// name the tab had, the text the field started with, and how the rename ends.
#[derive(Debug, Clone)]
struct RenameSession {
    tab: TabId,
    field: Entity,
    label: Entity,
    name: Option<String>,
    /// The text the field started with.
    initial: String,
    /// `Some(true)` to commit and `Some(false)` to discard at the next
    /// [`RenameSystems::Finish`]; `None` while editing.
    ending: Option<bool>,
}

/// Replaces the tab's label with a focused text field that holds the tab's
/// name, or the label it shows when it has none, all of it selected, and
/// draws a caret in the label's text color and a highlight behind the
/// selection; releases webview focus.
fn start_rename(
    ev: On<StartTabRename>,
    mut commands: Commands,
    mut rename: ResMut<TabRename>,
    mut labels: Query<(&mut Node, Option<&Children>), With<TabLabel>>,
    texts: Query<&Text, With<TabLabelText>>,
    tabs: Res<CurrentTabs>,
    buttons: Query<(Entity, &TabButton, &Children)>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    if rename.is_active() {
        return;
    }
    let Some(tab) = ev.tab.or(tabs.active) else {
        return;
    };
    let Some(position) = tabs.position_of(tab) else {
        return;
    };
    let name = tabs
        .entries
        .get(position)
        .and_then(|entry| entry.name.clone());
    let Some((button, _, parts)) = buttons.iter().find(|(_, button, _)| button.id == tab) else {
        return;
    };
    let Some((slot, label)) = parts
        .iter()
        .enumerate()
        .find(|(_, part)| labels.contains(*part))
    else {
        return;
    };
    let shown = labels
        .get(label)
        .ok()
        .and_then(|(_, children)| children)
        .and_then(|children| children.iter().find_map(|text| texts.get(text).ok()))
        .map(|text| text.0.clone())
        .filter(|text| !text.is_empty());
    let initial = name
        .clone()
        .or(shown)
        .unwrap_or_else(|| tab_label(position, None, None));
    if let Ok((mut node, _)) = labels.get_mut(label)
        && node.display != Display::None
    {
        node.display = Display::None;
    }
    let field = commands
        .spawn((
            EditableText {
                max_characters: Some(Tab::MAX_NAME_CHARS),
                allow_newlines: false,
                cursor_blink_period: STEADY_CARET,
                ..EditableText::new(initial.clone())
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
    commands.entity(button).insert_children(slot, &[field]);
    commands.trigger(RequestWebviewFocus::new(None));
    rename.0 = Some(RenameSession {
        tab,
        field,
        label,
        name,
        initial,
        ending: None,
    });
}

/// Asks to commit on Enter and to cancel on Esc, outside IME compositions.
fn on_field_key(
    ev: On<FocusedInput<KeyboardInput>>,
    mut rename: ResMut<TabRename>,
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
fn commit_on_outside_press(ev: On<Pointer<Press>>, mut rename: ResMut<TabRename>) {
    let origin = ev.original_event_target();
    if ev.entity != origin || rename.field() == Some(origin) || !rename.can_end() {
        return;
    }
    rename.request_end(true);
}

/// Asks to commit when a webview takes the keyboard focus.
fn commit_on_webview_focus(mut rename: ResMut<TabRename>, focused: Res<FocusedWebview>) {
    if focused.0.is_some() && rename.can_end() {
        rename.request_end(true);
    }
}

/// Commits or discards the ending rename, restores the label, removes the
/// field, and takes the keyboard focus off it. A committed name is sent as
/// a rename request; the tab bar shows it in the same frame.
fn finish_rename(
    mut commands: Commands,
    mut rename: ResMut<TabRename>,
    mut input_focus: ResMut<InputFocus>,
    mut labels: Query<&mut Node, With<TabLabel>>,
    fields: Query<&EditableText>,
) {
    let Some(session) = rename.0.take() else {
        return;
    };
    let committed = fields
        .get(session.field)
        .ok()
        .filter(|_| session.ending == Some(true))
        .and_then(|field| {
            match RenameOutcome::decide(
                &field.value().to_string(),
                session.name.as_deref(),
                &session.initial,
            ) {
                RenameOutcome::Set(name) => Some(name),
                RenameOutcome::Keep => None,
            }
        });
    if let Ok(mut node) = labels.get_mut(session.label)
        && node.display == Display::None
    {
        node.display = Display::Flex;
    }
    if let Some(name) = committed {
        commands.trigger(RequestTabAction {
            action: TabAction::Rename {
                tab: session.tab,
                name,
            },
        });
    }
    commands.entity(session.field).try_despawn();
    if input_focus.get() == Some(session.field) {
        input_focus.clear();
    }
}

/// Whether a rename waits to be finished.
fn rename_is_ending(rename: Res<TabRename>) -> bool {
    rename.is_ending()
}

/// Drops, without a request, a rename whose tab closed, and takes
/// the keyboard focus off its field.
fn drop_rename_of_closed_tab(
    mut rename: ResMut<TabRename>,
    mut input_focus: ResMut<InputFocus>,
    tabs: Res<CurrentTabs>,
) {
    let Some(tab) = rename.tab() else {
        return;
    };
    if tabs.position_of(tab).is_some() {
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
    use bevy_orzmux::prelude::TabEntry;
    use orzmux::prelude::PaneId;

    /// Asserts the commit rules: blank restores the automatic name, the
    /// untouched automatic label or the same name keeps things as they
    /// are, and anything else sets the name.
    ///
    /// Case: the user opens the rename field on tabs with and without a
    /// name and confirms various inputs.
    #[test]
    fn the_commit_rules_decide_the_name() {
        assert_eq!(
            RenameOutcome::decide("  ", Some("logs"), "Tab 2"),
            RenameOutcome::Set(None)
        );
        assert_eq!(
            RenameOutcome::decide("", None, "Tab 2"),
            RenameOutcome::Keep
        );
        assert_eq!(
            RenameOutcome::decide("Tab 2", None, "Tab 2"),
            RenameOutcome::Keep
        );
        assert_eq!(
            RenameOutcome::decide("logs", Some("logs"), "Tab 2"),
            RenameOutcome::Keep
        );
        assert_eq!(
            RenameOutcome::decide(" build ", None, "Tab 2"),
            RenameOutcome::Set(Some("build".into()))
        );
    }

    /// Asserts that an unnamed tab stays unnamed when its field is
    /// committed as it started, whether it started as `Tab n` or as a
    /// title, even one longer than a name may be.
    ///
    /// Case: the user opens the rename field on a tab showing vim's long
    /// title and presses Enter without typing.
    #[test]
    fn an_untitled_or_titled_field_left_untouched_keeps_the_tab_unnamed() {
        assert_eq!(
            RenameOutcome::decide("Tab 2", None, "Tab 2"),
            RenameOutcome::Keep
        );
        assert_eq!(
            RenameOutcome::decide("vim", None, "vim"),
            RenameOutcome::Keep
        );
        let long = "x".repeat(Tab::MAX_NAME_CHARS + 10);
        assert_eq!(
            RenameOutcome::decide(&long, None, &long),
            RenameOutcome::Keep
        );
        assert_eq!(
            RenameOutcome::decide("vim", Some("logs"), "logs"),
            RenameOutcome::Set(Some("vim".into()))
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
    struct Requested(Vec<TabAction>);

    fn labelled(app: &mut App, shown: &str) -> (Entity, Entity) {
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
        let text = app
            .world_mut()
            .spawn((TabLabelText, Text::new(shown), ChildOf(label)))
            .id();
        (label, text)
    }

    /// Asserts that a cancelled rename leaves the label's text as it was.
    ///
    /// Case: the user types "logs" into the rename field and presses Esc.
    #[test]
    fn a_cancelled_rename_keeps_the_label_text() {
        let mut app = finish_app();
        let (label, text) = labelled(&mut app, "Tab 1");
        let field = app.world_mut().spawn(EditableText::new("logs")).id();
        app.insert_resource(TabRename::ending_for_test(TabId(1), field, label, false));
        app.update();
        assert_eq!(
            app.world().get::<Text>(text).map(|t| t.0.as_str()),
            Some("Tab 1")
        );
    }

    fn finish_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputFocus>()
            .init_resource::<Requested>()
            .add_observer(|ev: On<RequestTabAction>, mut seen: ResMut<Requested>| {
                seen.0.push(ev.action.clone());
            })
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
        app.insert_resource(TabRename::ending_for_test(TabId(1), field, label, true));
        app.update();
        assert_eq!(
            app.world().resource::<Requested>().0,
            vec![TabAction::Rename {
                tab: TabId(1),
                name: Some("logs".into())
            }]
        );
        assert!(!app.world().resource::<TabRename>().is_active());
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
        app.insert_resource(TabRename::ending_for_test(TabId(1), field, label, false));
        app.update();
        assert!(app.world().resource::<Requested>().0.is_empty());
        assert!(!app.world().resource::<TabRename>().is_active());
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
        app.insert_resource(TabRename(Some(RenameSession {
            tab: TabId(1),
            field,
            label: Entity::PLACEHOLDER,
            name: None,
            initial: "Tab 1".into(),
            ending: None,
        })));

        app.world_mut().trigger(press_on(field));
        app.update();

        assert!(app.world().resource::<TabRename>().is_active());
        assert!(app.world().resource::<Requested>().0.is_empty());

        app.world_mut().trigger(press_on(pane));
        app.update();

        assert!(!app.world().resource::<TabRename>().is_active());
        assert_eq!(
            app.world().resource::<Requested>().0,
            vec![TabAction::Rename {
                tab: TabId(1),
                name: Some("build".into())
            }]
        );
    }

    /// Asserts that a rename of a tab that closes meanwhile is
    /// dropped without a request.
    ///
    /// Case: the renamed tab's last shell exits while the user types.
    #[test]
    fn a_rename_of_a_closed_tab_is_dropped() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputFocus>()
            .init_resource::<CurrentTabs>()
            .insert_resource(TabRename::active_for_test(TabId(1)))
            .add_systems(
                Update,
                drop_rename_of_closed_tab.run_if(resource_exists_and_changed::<CurrentTabs>),
            );
        app.world_mut().resource_mut::<CurrentTabs>().entries = vec![TabEntry {
            id: TabId(2),
            name: None,
            active_pane: PaneId(1),
        }];
        app.update();
        assert!(!app.world().resource::<TabRename>().is_active());
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
        app.init_resource::<TabRename>()
            .init_resource::<CurrentTabs>()
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
            let mut tabs = app.world_mut().resource_mut::<CurrentTabs>();
            tabs.entries = vec![TabEntry {
                id: TabId(1),
                name: None,
                active_pane: PaneId(1),
            }];
            tabs.active = Some(TabId(1));
        }
        let tab = app.world_mut().spawn(TabButton { id: TabId(1) }).id();
        app.world_mut()
            .spawn((TabLabel, Node::default(), ChildOf(tab)));
        let page = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(page);
        app.update();

        app.world_mut().trigger(StartTabRename { tab: None });
        app.update();

        assert_eq!(app.world().resource::<FocusedWebview>().0, None);
        assert!(app.world().resource::<TabRename>().can_end());
        let field = app
            .world()
            .resource::<TabRename>()
            .field()
            .expect("the rename has a field");
        app.world_mut()
            .entity_mut(field)
            .insert(EditableText::new("build"));

        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(page);
        app.update();

        assert!(!app.world().resource::<TabRename>().is_active());
        assert_eq!(
            app.world().resource::<Requested>().0,
            vec![TabAction::Rename {
                tab: TabId(1),
                name: Some("build".into())
            }]
        );
    }
}
