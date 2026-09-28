//! The workspace tab bar across the top of the window: its node, its
//! height, and the tabs it lists.

use crate::font::TerminalUiFont;
use crate::session::spawn::PaneSpawnRequest;
use crate::ui::UiRoot;
use bevy::input::mouse::MouseScrollUnit;
use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy::ui_widgets::{ScrollArea, ScrollIntoView};
use bevy::window::{PrimaryWindow, WindowScaleFactorChanged};
use bevy_orzmux::prelude::{
    CloseTarget, CurrentWorkspaces, OrzmuxSystems, RequestWorkspaceAction, WorkspaceAction,
    WorkspaceEntry, WorkspaceId, WorkspaceTarget,
};
use orzmux::prelude::NewPaneAt;
use std::collections::HashMap;

/// The tab bar's height in logical px before rounding to physical pixels.
pub(crate) const TAB_BAR_HEIGHT_PX: f32 = 28.0;

/// Ordering slots for the tab bar's `Update` systems: `Reconcile` runs
/// before `Style`, and both run after `OrzmuxSystems::Drain`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum TabBarSystems {
    /// Spawning, removing, and ordering the tabs.
    Reconcile,
    /// Restyling the tabs.
    Style,
}

/// The workspace tab bar's root node, the first child of `UiRoot`.
#[derive(Component)]
pub(crate) struct TabBar;

/// The scroll container the tabs and the new-workspace button live in.
#[derive(Component)]
pub(crate) struct TabStrip;

/// One workspace's tab.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkspaceTab {
    /// The workspace the tab stands for.
    pub workspace: WorkspaceId,
}

/// A tab's label text.
#[derive(Component)]
pub(crate) struct TabLabel;

/// Spawns the tab bar, keeps its height on whole physical pixels, and
/// keeps one clickable tab per workspace in it.
pub(crate) struct TabBarPlugin;

impl Plugin for TabBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<WindowScaleFactorChanged>()
            .configure_sets(
                Update,
                (TabBarSystems::Reconcile, TabBarSystems::Style)
                    .chain()
                    .after(OrzmuxSystems::Drain),
            )
            .add_systems(
                Update,
                (
                    ensure_tab_bar
                        .run_if(not(any_with_component::<TabBar>))
                        .before(TabBarSystems::Reconcile),
                    size_tab_bar.run_if(on_message::<WindowScaleFactorChanged>),
                    reconcile_tabs.in_set(TabBarSystems::Reconcile).run_if(
                        resource_exists_and_changed::<CurrentWorkspaces>
                            .or_else(any_match_filter::<Added<TabStrip>>),
                    ),
                    style_tabs.in_set(TabBarSystems::Style).run_if(
                        resource_exists_and_changed::<CurrentWorkspaces>
                            .or_else(any_match_filter::<Added<TabHovered>>)
                            .or_else(any_component_removed::<TabHovered>)
                            .or_else(any_match_filter::<Added<WorkspaceTab>>),
                    ),
                ),
            )
            .add_systems(
                PostUpdate,
                scroll_active_tab_into_view
                    .after(UiSystems::Layout)
                    .run_if(resource_exists_and_changed::<CurrentWorkspaces>),
            );
    }
}

/// The tab bar's height in whole physical pixels at `scale_factor`.
pub(crate) fn tab_bar_height_phys(scale_factor: f32) -> u32 {
    (TAB_BAR_HEIGHT_PX * scale_factor).round().max(0.0) as u32
}

/// The text a tab shows at zero-based `position`: its name, or
/// `Workspace {position + 1}`.
pub(crate) fn tab_label(position: usize, name: Option<&str>) -> String {
    match name {
        Some(name) => name.to_string(),
        None => format!("Workspace {}", position + 1),
    }
}

/// The workspace ids in the order the tabs show them.
pub(crate) fn tab_order(entries: &[WorkspaceEntry]) -> Vec<WorkspaceId> {
    entries.iter().map(|entry| entry.id).collect()
}

/// The bar's background.
const BAR_BG: Color = Color::srgb_u8(0x14, 0x15, 0x18);
/// The line under the bar and between inactive tabs.
const BAR_LINE: Color = Color::srgb_u8(0x59, 0x59, 0x66);
/// The narrowest a tab gets before the strip scrolls, in logical px.
const TAB_MIN_WIDTH_PX: f32 = 80.0;
/// The widest a tab gets, in logical px.
const TAB_MAX_WIDTH_PX: f32 = 240.0;
/// The width of the new-workspace button, in logical px.
const NEW_BUTTON_WIDTH_PX: f32 = 36.0;
/// The font size of the tab bar's text, in logical px.
const TAB_FONT_PX: f32 = 12.0;
/// The displayed tab's background.
const ACTIVE_BG: Color = Color::srgb_u8(0x00, 0x00, 0x00);
/// The displayed tab's top edge.
const ACTIVE_ACCENT: Color = Color::srgb_u8(0xf2, 0xd9, 0x33);
/// The displayed tab's text.
const ACTIVE_TEXT: Color = Color::srgb_u8(0xff, 0xff, 0xff);
/// The text of the other tabs and of the new-workspace button.
const INACTIVE_TEXT: Color = Color::srgb_u8(0xa0, 0xa0, 0xac);
/// The background of a hovered tab that is not displayed.
const HOVER_BG: Color = Color::srgb_u8(0x1c, 0x1d, 0x22);

/// The 1 px line along the bar's bottom edge. It is the bar's first child,
/// so the tabs paint over it and the displayed tab's opaque background
/// hides it under that tab.
#[derive(Component)]
struct TabBarLine;

/// A tab's close button.
#[derive(Component)]
struct TabClose;

/// The divider after a tab, shown between two tabs that are not displayed.
#[derive(Component)]
struct TabDivider;

/// The new-workspace button.
#[derive(Component)]
struct NewWorkspaceButton;

/// Marks a tab under the pointer.
#[derive(Component)]
struct TabHovered;

/// The bar's height in logical px that lands on whole physical pixels.
fn tab_bar_height_logical(scale_factor: f32) -> f32 {
    tab_bar_height_phys(scale_factor) as f32 / scale_factor.max(f32::EPSILON)
}

/// The tab bar's text face: the UI font, or the default face when none is
/// loaded, at the tab font size.
fn tab_font(ui_font: Option<&TerminalUiFont>) -> TextFont {
    let size = FontSize::Px(TAB_FONT_PX);
    ui_font.map_or_else(
        || TerminalUiFont::default().text_font(size),
        |font| font.text_font(size),
    )
}

/// Spawns the bar as the first child of `UiRoot`, above the shell surface,
/// with the tab strip in it.
fn ensure_tab_bar(
    mut commands: Commands,
    ui_root: Query<Entity, With<UiRoot>>,
    window: Query<&Window, With<PrimaryWindow>>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let Ok(ui_root) = ui_root.single() else {
        return;
    };
    let scale = window.single().map(Window::scale_factor).unwrap_or(1.0);
    let bar = commands
        .spawn((
            Name::new("Tab Bar"),
            TabBar,
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(tab_bar_height_logical(scale)),
                flex_shrink: 0.0,
                ..default()
            },
            BackgroundColor(BAR_BG),
        ))
        .id();
    commands.spawn((
        TabBarLine,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            bottom: Val::Px(0.0),
            height: Val::Px(1.0),
            ..default()
        },
        BackgroundColor(BAR_LINE),
        ChildOf(bar),
    ));
    spawn_tab_strip(&mut commands, bar, &tab_font(ui_font.as_deref()));
    commands.entity(ui_root).insert_children(0, &[bar]);
}

/// Spawns the horizontally scrolling strip under `bar`, holding only the
/// new-workspace button.
fn spawn_tab_strip(commands: &mut Commands, bar: Entity, font: &TextFont) {
    let strip = commands
        .spawn((
            TabStrip,
            ScrollArea,
            Node {
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                height: Val::Percent(100.0),
                overflow: Overflow::scroll_x(),
                ..default()
            },
            ChildOf(bar),
        ))
        .observe(scroll_strip_vertically)
        .id();
    commands
        .spawn((
            NewWorkspaceButton,
            Node {
                width: Val::Px(NEW_BUTTON_WIDTH_PX),
                flex_shrink: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            ChildOf(strip),
        ))
        .with_child((Text::new("+"), font.clone(), TextColor(INACTIVE_TEXT)))
        .observe(on_new_workspace_click);
}

/// Re-applies the bar's height after the window's scale factor changes.
fn size_tab_bar(
    mut bars: Query<&mut Node, With<TabBar>>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let height = Val::Px(tab_bar_height_logical(window.scale_factor()));
    for mut node in &mut bars {
        if node.height != height {
            node.height = height;
        }
    }
}

/// Spawns a tab for each new workspace, despawns the tabs of closed ones,
/// updates labels, and orders the strip's children as `tab_order` says,
/// keeping the new-workspace button last.
fn reconcile_tabs(
    mut commands: Commands,
    mut labels: Query<&mut Text, With<TabLabel>>,
    workspaces: Res<CurrentWorkspaces>,
    ui_font: Option<Res<TerminalUiFont>>,
    tabs: Query<(Entity, &WorkspaceTab, Option<&Children>)>,
    strip: Query<(Entity, Option<&Children>), With<TabStrip>>,
    plus: Query<Entity, With<NewWorkspaceButton>>,
) {
    let Ok((strip, strip_children)) = strip.single() else {
        return;
    };
    let mut by_id: HashMap<WorkspaceId, Entity> = HashMap::new();
    for (entity, tab, parts) in &tabs {
        let Some(position) = workspaces.position_of(tab.workspace) else {
            commands.entity(entity).despawn();
            continue;
        };
        let name = workspaces
            .entries
            .get(position)
            .and_then(|entry| entry.name.as_deref());
        let label = tab_label(position, name);
        for part in parts.into_iter().flatten() {
            if let Ok(mut text) = labels.get_mut(*part)
                && text.0 != label
            {
                text.0.clone_from(&label);
            }
        }
        by_id.insert(tab.workspace, entity);
    }
    let font = tab_font(ui_font.as_deref());
    for (position, entry) in workspaces.entries.iter().enumerate() {
        if by_id.contains_key(&entry.id) {
            continue;
        }
        let label = tab_label(position, entry.name.as_deref());
        let tab = spawn_tab(&mut commands, strip, entry.id, label, &font);
        by_id.insert(entry.id, tab);
    }
    let mut ordered: Vec<Entity> = tab_order(&workspaces.entries)
        .into_iter()
        .filter_map(|id| by_id.get(&id).copied())
        .collect();
    ordered.extend(plus.iter());
    if strip_children.map(|children| &children[..]) != Some(&ordered[..]) {
        commands.entity(strip).replace_children(&ordered);
    }
}

/// Spawns one tab with its label, close button, and divider under `strip`,
/// and returns it.
fn spawn_tab(
    commands: &mut Commands,
    strip: Entity,
    workspace: WorkspaceId,
    label: String,
    font: &TextFont,
) -> Entity {
    let tab = commands
        .spawn((
            WorkspaceTab { workspace },
            Node {
                flex_grow: 1.0,
                flex_basis: Val::Px(TAB_MAX_WIDTH_PX),
                min_width: Val::Px(TAB_MIN_WIDTH_PX),
                max_width: Val::Px(TAB_MAX_WIDTH_PX),
                height: Val::Percent(100.0),
                padding: UiRect::horizontal(Val::Px(12.0)),
                column_gap: Val::Px(8.0),
                align_items: AlignItems::Center,
                border: UiRect::top(Val::Px(2.0)),
                ..default()
            },
            ChildOf(strip),
        ))
        .observe(on_tab_click)
        .observe(on_tab_enter)
        .observe(on_tab_leave)
        .id();
    commands.spawn((
        TabLabel,
        Text::new(label),
        font.clone(),
        TextColor(INACTIVE_TEXT),
        TextLayout::no_wrap(),
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(0.0),
            overflow: Overflow::clip_x(),
            ..default()
        },
        ChildOf(tab),
    ));
    commands
        .spawn((
            TabClose,
            Text::new("×"),
            font.clone(),
            TextColor(INACTIVE_TEXT),
            Node {
                display: Display::None,
                ..default()
            },
            ChildOf(tab),
        ))
        .observe(on_close_click);
    commands.spawn((
        TabDivider,
        BackgroundColor(BAR_LINE),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(0.0),
            top: Val::Px(7.0),
            width: Val::Px(1.0),
            height: Val::Px(14.0),
            display: Display::None,
            ..default()
        },
        ChildOf(tab),
    ));
    tab
}

/// Colors each tab for whether it is displayed or hovered, shows the close
/// button on the displayed tab and on a hovered one, and shows a tab's
/// divider only when neither it nor the next tab is displayed.
fn style_tabs(
    mut tabs: Query<(
        &WorkspaceTab,
        &mut BackgroundColor,
        &mut BorderColor,
        &Children,
        Has<TabHovered>,
    )>,
    mut texts: Query<&mut TextColor, Or<(With<TabLabel>, With<TabClose>)>>,
    mut closes: Query<&mut Node, (With<TabClose>, Without<TabDivider>)>,
    mut dividers: Query<&mut Node, (With<TabDivider>, Without<TabClose>)>,
    workspaces: Res<CurrentWorkspaces>,
) {
    let order = tab_order(&workspaces.entries);
    for (tab, mut background, mut border, parts, hovered) in &mut tabs {
        let displayed = workspaces.active == Some(tab.workspace);
        let next = order.iter().skip_while(|id| **id != tab.workspace).nth(1);
        let divided = !displayed && next.is_some_and(|id| workspaces.active != Some(*id));
        let fill = if displayed {
            ACTIVE_BG
        } else if hovered {
            HOVER_BG
        } else {
            Color::NONE
        };
        let accent = if displayed {
            ACTIVE_ACCENT
        } else {
            Color::NONE
        };
        let text = TextColor(if displayed {
            ACTIVE_TEXT
        } else {
            INACTIVE_TEXT
        });
        let close = if displayed || hovered {
            Display::Flex
        } else {
            Display::None
        };
        let divider = if divided {
            Display::Flex
        } else {
            Display::None
        };
        background.set_if_neq(BackgroundColor(fill));
        border.set_if_neq(BorderColor {
            top: accent,
            ..BorderColor::DEFAULT
        });
        for part in parts.iter() {
            if let Ok(mut color) = texts.get_mut(part) {
                color.set_if_neq(text);
            }
            if let Ok(mut node) = closes.get_mut(part)
                && node.display != close
            {
                node.display = close;
            }
            if let Ok(mut node) = dividers.get_mut(part)
                && node.display != divider
            {
                node.display = divider;
            }
        }
    }
}

/// A primary click on a tab requests that its workspace be displayed.
fn on_tab_click(ev: On<Pointer<Click>>, mut commands: Commands, tabs: Query<&WorkspaceTab>) {
    if ev.button != PointerButton::Primary {
        return;
    }
    let Ok(tab) = tabs.get(ev.entity) else {
        return;
    };
    commands.trigger(RequestWorkspaceAction {
        action: WorkspaceAction::Select(WorkspaceTarget::Id(tab.workspace)),
    });
}

/// A primary click on a close button requests its tab's close; no click on
/// it reaches the tab.
fn on_close_click(
    mut ev: On<Pointer<Click>>,
    mut commands: Commands,
    parents: Query<&ChildOf>,
    tabs: Query<&WorkspaceTab>,
) {
    ev.propagate(false);
    if ev.button != PointerButton::Primary {
        return;
    }
    let Some(tab) = parents
        .get(ev.entity)
        .ok()
        .and_then(|parent| tabs.get(parent.parent()).ok())
    else {
        return;
    };
    commands.trigger(RequestWorkspaceAction {
        action: WorkspaceAction::Close(CloseTarget::Id(tab.workspace)),
    });
}

/// A primary click on the plus button requests a new workspace.
fn on_new_workspace_click(ev: On<Pointer<Click>>, mut commands: Commands) {
    if ev.button == PointerButton::Primary {
        commands.trigger(PaneSpawnRequest {
            at: NewPaneAt::Workspace,
        });
    }
}

/// Marks the tab the pointer entered as hovered.
fn on_tab_enter(ev: On<Pointer<Enter>>, mut commands: Commands) {
    commands.entity(ev.entity).try_insert(TabHovered);
}

/// Clears the hover mark of the tab the pointer left.
fn on_tab_leave(ev: On<Pointer<Leave>>, mut commands: Commands) {
    commands.entity(ev.entity).try_remove::<TabHovered>();
}

/// Maps a purely vertical wheel over the strip to horizontal scrolling,
/// clamped to the strip's overflow; a wheel with horizontal travel is left
/// alone.
fn scroll_strip_vertically(
    ev: On<Pointer<Scroll>>,
    mut strips: Query<(&mut ScrollPosition, &ComputedNode), With<TabStrip>>,
) {
    if ev.x != 0.0 || ev.y == 0.0 {
        return;
    }
    let Ok((mut position, node)) = strips.get_mut(ev.entity) else {
        return;
    };
    let unit = match ev.unit {
        MouseScrollUnit::Line => MouseScrollUnit::SCROLL_UNIT_CONVERSION_FACTOR,
        MouseScrollUnit::Pixel => 1.0,
    };
    let max = ((node.content_size().x - node.size().x) * node.inverse_scale_factor).max(0.0);
    let next = (position.x - ev.y * unit).clamp(0.0, max);
    if position.x != next {
        position.x = next;
    }
}

/// Scrolls the strip so the displayed workspace's tab is fully visible.
fn scroll_active_tab_into_view(
    mut commands: Commands,
    workspaces: Res<CurrentWorkspaces>,
    tabs: Query<(Entity, &WorkspaceTab)>,
) {
    let Some(active) = workspaces.active else {
        return;
    };
    if let Some((entity, _)) = tabs.iter().find(|(_, tab)| tab.workspace == active) {
        commands.trigger(ScrollIntoView { entity });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::{Location, PointerId};
    use std::time::Duration;

    /// A request the tab bar sent, in the order it was sent.
    #[derive(Debug, PartialEq)]
    enum Sent {
        Workspace(WorkspaceAction),
        Spawn(NewPaneAt),
    }

    #[derive(Resource, Default)]
    struct SentRequests(Vec<Sent>);

    fn app_with_tab_bar() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TabBarPlugin))
            .init_resource::<CurrentWorkspaces>();
        app.world_mut().spawn((Node::default(), UiRoot));
        app
    }

    fn set_workspaces(app: &mut App, ids: &[u32], active: u32) {
        let mut workspaces = app.world_mut().resource_mut::<CurrentWorkspaces>();
        workspaces.entries = ids
            .iter()
            .map(|id| WorkspaceEntry {
                id: WorkspaceId(*id),
                name: None,
            })
            .collect();
        workspaces.active = Some(WorkspaceId(active));
    }

    fn strip_children(app: &mut App) -> Vec<Entity> {
        let world = app.world_mut();
        world
            .query_filtered::<&Children, With<TabStrip>>()
            .single(world)
            .expect("exactly one tab strip")
            .to_vec()
    }

    /// The strip's tabs in child order, each with its label text.
    fn listed_tabs(app: &mut App) -> Vec<(WorkspaceId, String)> {
        let children = strip_children(app);
        let world = app.world();
        children
            .iter()
            .filter_map(|child| {
                let tab = world.get::<WorkspaceTab>(*child)?;
                let label = world
                    .get::<Children>(*child)?
                    .iter()
                    .filter(|part| world.get::<TabLabel>(*part).is_some())
                    .find_map(|part| world.get::<Text>(part))?;
                Some((tab.workspace, label.0.clone()))
            })
            .collect()
    }

    fn tab_of(app: &mut App, workspace: u32) -> Entity {
        let world = app.world_mut();
        world
            .query::<(Entity, &WorkspaceTab)>()
            .iter(world)
            .find(|(_, tab)| tab.workspace == WorkspaceId(workspace))
            .map(|(entity, _)| entity)
            .expect("the workspace has a tab")
    }

    fn close_button_of(app: &mut App, tab: Entity) -> Entity {
        let world = app.world();
        world
            .get::<Children>(tab)
            .expect("the tab has parts")
            .iter()
            .find(|part| world.get::<TabClose>(*part).is_some())
            .expect("the tab has a close button")
    }

    fn new_workspace_button(app: &mut App) -> Entity {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<NewWorkspaceButton>>()
            .single(world)
            .expect("exactly one new-workspace button")
    }

    fn click(entity: Entity) -> Pointer<Click> {
        Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 800,
                    height: 600,
                },
                position: Vec2::ZERO,
            },
            Click {
                button: PointerButton::Primary,
                hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
                duration: Duration::ZERO,
                count: 1,
            },
            entity,
        )
    }

    /// Asserts that an unnamed tab shows `Workspace n` by position and a
    /// named tab shows its name.
    ///
    /// Case: the user renamed the second of three tabs.
    #[test]
    fn labels_follow_the_position_unless_named() {
        assert_eq!(tab_label(0, None), "Workspace 1");
        assert_eq!(tab_label(1, Some("logs")), "logs");
        assert_eq!(tab_label(2, None), "Workspace 3");
    }

    /// Asserts that one tab per workspace exists in display order, and
    /// that a closed workspace's tab is removed.
    ///
    /// Case: the user opens three workspaces, then closes the middle one.
    #[test]
    fn tabs_follow_the_workspace_list() {
        let mut app = app_with_tab_bar();
        set_workspaces(&mut app, &[1, 2, 3], 1);
        app.update();
        app.update();

        assert_eq!(
            listed_tabs(&mut app),
            vec![
                (WorkspaceId(1), "Workspace 1".to_string()),
                (WorkspaceId(2), "Workspace 2".to_string()),
                (WorkspaceId(3), "Workspace 3".to_string()),
            ]
        );
        let plus = new_workspace_button(&mut app);
        assert_eq!(strip_children(&mut app).last(), Some(&plus));

        set_workspaces(&mut app, &[1, 3], 1);
        app.update();

        assert_eq!(
            listed_tabs(&mut app),
            vec![
                (WorkspaceId(1), "Workspace 1".to_string()),
                (WorkspaceId(3), "Workspace 2".to_string()),
            ]
        );
        assert_eq!(strip_children(&mut app).last(), Some(&plus));
        let world = app.world_mut();
        assert_eq!(
            world.query::<&WorkspaceTab>().iter(world).count(),
            2,
            "the closed workspace's tab is despawned"
        );
    }

    /// Asserts that clicking a tab requests its selection, clicking its
    /// close button requests its close without also selecting it, and the
    /// plus button requests a new workspace.
    ///
    /// Case: the user clicks the second tab, then the first tab's ×, then +.
    #[test]
    fn clicks_become_workspace_requests() {
        let mut app = app_with_tab_bar();
        app.init_resource::<SentRequests>()
            .add_observer(
                |ev: On<RequestWorkspaceAction>, mut sent: ResMut<SentRequests>| {
                    sent.0.push(Sent::Workspace(ev.action.clone()));
                },
            )
            .add_observer(|ev: On<PaneSpawnRequest>, mut sent: ResMut<SentRequests>| {
                sent.0.push(Sent::Spawn(ev.at));
            });
        set_workspaces(&mut app, &[1, 2], 1);
        app.update();
        app.update();

        let second = tab_of(&mut app, 2);
        let first = tab_of(&mut app, 1);
        let close_first = close_button_of(&mut app, first);
        let plus = new_workspace_button(&mut app);
        app.world_mut().trigger(click(second));
        app.world_mut().trigger(click(close_first));
        app.world_mut().trigger(click(plus));
        app.update();

        assert_eq!(
            app.world().resource::<SentRequests>().0,
            vec![
                Sent::Workspace(WorkspaceAction::Select(WorkspaceTarget::Id(WorkspaceId(2)))),
                Sent::Workspace(WorkspaceAction::Close(CloseTarget::Id(WorkspaceId(1)))),
                Sent::Spawn(NewPaneAt::Workspace),
            ]
        );
    }

    /// Asserts that the bar's physical height is 28 logical px rounded to
    /// whole physical pixels.
    ///
    /// Case: orzma runs on a standard display, a 125% Windows display, and
    /// a Retina display.
    #[test]
    fn the_bar_height_rounds_to_whole_physical_pixels() {
        assert_eq!(tab_bar_height_phys(1.0), 28);
        assert_eq!(tab_bar_height_phys(1.25), 35);
        assert_eq!(tab_bar_height_phys(1.1), 31);
        assert_eq!(tab_bar_height_phys(2.0), 56);
    }
}
