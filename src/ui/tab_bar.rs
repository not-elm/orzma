//! The tab bar across the top of the window: its node, its
//! height, and the tabs it lists.

use crate::font::TerminalUiFont;
use crate::session::spawn::PaneSpawnRequest;
use crate::ui::UiRoot;
use crate::ui::tab_bar::drag::{TabDrag, TabDragPlugin};
use crate::ui::tab_bar::rename::{StartTabRename, TabRename, TabRenamePlugin};
use bevy::input::mouse::MouseScrollUnit;
use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy::ui_widgets::{ScrollArea, ScrollIntoView};
use bevy::window::{PrimaryWindow, WindowScaleFactorChanged};
use bevy_orzmux::prelude::{
    CloseTarget, CurrentTabs, OrzmuxSystems, RequestTabAction, TabAction, TabEntry, TabId,
    TabTarget,
};
use orzmux::prelude::NewPaneAt;
use std::collections::HashMap;

mod drag;
pub(crate) mod rename;

/// Ordering slots for the tab bar's `Update` systems: `Reconcile` runs
/// before `Style`, and both run after `OrzmuxSystems::Drain`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum TabBarSystems {
    /// Spawning, removing, and ordering the tabs.
    Reconcile,
    /// Restyling the tabs.
    Style,
}

/// The tab bar's root node, the first child of `UiRoot`.
#[derive(Component)]
pub(crate) struct TabBar;

/// The scroll container the tabs and the new-tab button live in.
#[derive(Component)]
pub(crate) struct TabStrip;

/// The button that stands for one tab in the tab bar.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TabButton {
    /// The tab the button stands for.
    pub id: TabId,
}

/// A tab's label: a direct child of the tab that holds the label text and
/// clips it at its own edges, so hiding this node hides the whole label.
#[derive(Component)]
pub(crate) struct TabLabel;

/// Spawns the tab bar, keeps its height on whole physical pixels, and
/// keeps one clickable button per tab in it.
pub(crate) struct TabBarPlugin;

impl Plugin for TabBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((TabDragPlugin, TabRenamePlugin))
            .add_message::<WindowScaleFactorChanged>()
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
                        resource_exists_and_changed::<CurrentTabs>
                            .or_else(any_match_filter::<Added<TabStrip>>)
                            .or_else(resource_exists_and_changed::<TabDrag>),
                    ),
                    style_tabs.in_set(TabBarSystems::Style).run_if(
                        resource_exists_and_changed::<CurrentTabs>
                            .or_else(any_match_filter::<Added<TabHovered>>)
                            .or_else(any_component_removed::<TabHovered>)
                            .or_else(any_match_filter::<Added<TabButton>>),
                    ),
                ),
            )
            .add_systems(
                PostUpdate,
                scroll_active_tab_into_view
                    .after(UiSystems::Layout)
                    .run_if(resource_exists_and_changed::<CurrentTabs>),
            );
    }
}

/// The tab bar's height in whole physical pixels at `scale_factor`.
pub(crate) fn tab_bar_height_phys(scale_factor: f32) -> u32 {
    (TAB_BAR_HEIGHT_PX * scale_factor).round().max(0.0) as u32
}

/// The text a tab shows at zero-based `position`: its name, or
/// `Tab {position + 1}`.
pub(crate) fn tab_label(position: usize, name: Option<&str>) -> String {
    match name {
        Some(name) => name.to_string(),
        None => format!("Tab {}", position + 1),
    }
}

/// The tab ids in the order the tabs show them: the backend's order,
/// with a dragged or just-dropped tab moved to its slot. A slot past the
/// end puts the tab last.
pub(crate) fn tab_order(entries: &[TabEntry], preview: Option<(TabId, usize)>) -> Vec<TabId> {
    let mut order: Vec<TabId> = entries.iter().map(|entry| entry.id).collect();
    if let Some((dragged, slot)) = preview
        && let Some(from) = order.iter().position(|id| *id == dragged)
    {
        let id = order.remove(from);
        order.insert(slot.min(order.len()), id);
    }
    order
}

/// The tab bar's height in logical px before rounding to physical pixels.
const TAB_BAR_HEIGHT_PX: f32 = 28.0;
/// The space between two tabs, in logical px.
const TAB_GAP_PX: f32 = 4.0;
/// The space before the first tab in the strip, in logical px.
const TAB_STRIP_LEFT_PADDING_PX: f32 = 6.0;
/// The font size of the tab bar's text, in logical px.
const TAB_FONT_PX: f32 = 12.0;
/// The displayed tab's background.
const ACTIVE_BG: Color = Color::srgb_u8(0x7c, 0x3a, 0xed);
/// The displayed tab's text.
const ACTIVE_TEXT: Color = Color::srgb_u8(0xff, 0xff, 0xff);
/// The text of the other tabs, of their close buttons, and of the
/// new-tab button.
const INACTIVE_TEXT: Color = Color::srgb_u8(0x8c, 0x8c, 0x98);
/// The background of a hovered tab that is not displayed.
const HOVER_BG: Color = Color::srgb_u8(0x3e, 0x2e, 0x66);
/// The text of the displayed tab's close button.
const ACTIVE_CLOSE: Color = Color::srgb_u8(0xc4, 0xb5, 0xfd);

/// The text inside a tab's `TabLabel`.
#[derive(Component)]
struct TabLabelText;

/// A tab's close button.
#[derive(Component)]
struct TabClose;

/// The new-tab button.
#[derive(Component)]
struct NewTabButton;

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
/// with a 1 px line along its bottom edge and the tab strip in it.
fn ensure_tab_bar(
    mut commands: Commands,
    ui_root: Query<Entity, With<UiRoot>>,
    window: Query<&Window, With<PrimaryWindow>>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    /// The bar's background.
    const BAR_BG: Color = Color::srgb_u8(0x24, 0x29, 0x2c);
    /// The line along the bar's bottom edge.
    const BAR_LINE: Color = Color::srgb_u8(0x14, 0x15, 0x18);

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
                border: UiRect::bottom(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(BAR_BG),
            BorderColor::all(BAR_LINE),
        ))
        .id();
    spawn_tab_strip(&mut commands, bar, &tab_font(ui_font.as_deref()));
    commands.entity(ui_root).insert_children(0, &[bar]);
}

/// Spawns the horizontally scrolling strip under `bar`, holding only the
/// new-tab button.
fn spawn_tab_strip(commands: &mut Commands, bar: Entity, font: &TextFont) {
    /// The width of the new-tab button, in logical px.
    const NEW_BUTTON_WIDTH_PX: f32 = 36.0;

    let strip = commands
        .spawn((
            TabStrip,
            ScrollArea,
            Node {
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                height: Val::Percent(100.0),
                padding: UiRect::left(Val::Px(TAB_STRIP_LEFT_PADDING_PX)),
                column_gap: Val::Px(TAB_GAP_PX),
                overflow: Overflow::scroll_x(),
                ..default()
            },
            ChildOf(bar),
        ))
        .observe(scroll_strip_vertically)
        .id();
    commands
        .spawn((
            NewTabButton,
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
        .observe(on_new_tab_click);
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

/// Spawns a button for each new tab, despawns the buttons of closed ones,
/// and orders the strip's children and numbers the labels as `tab_order`
/// says with the drag's preview, keeping the new-tab button last.
fn reconcile_tabs(
    mut commands: Commands,
    mut labels: Query<&mut Text, With<TabLabelText>>,
    tabs: Res<CurrentTabs>,
    drag: Res<TabDrag>,
    ui_font: Option<Res<TerminalUiFont>>,
    buttons: Query<(Entity, &TabButton, Option<&Children>)>,
    label_boxes: Query<&Children, With<TabLabel>>,
    strip: Query<(Entity, Option<&Children>), With<TabStrip>>,
    plus: Query<Entity, With<NewTabButton>>,
) {
    let Ok((strip, strip_children)) = strip.single() else {
        return;
    };
    let order = tab_order(&tabs.entries, drag.preview());
    let mut by_id: HashMap<TabId, Entity> = HashMap::new();
    for (entity, tab, parts) in &buttons {
        let Some(position) = order.iter().position(|id| *id == tab.id) else {
            commands.entity(entity).despawn();
            continue;
        };
        let label = tab_label(position, tab_name(&tabs.entries, tab.id));
        let texts = parts
            .into_iter()
            .flat_map(|parts| label_texts(parts, &label_boxes));
        for text in texts {
            if let Ok(mut text) = labels.get_mut(text)
                && text.0 != label
            {
                text.0.clone_from(&label);
            }
        }
        by_id.insert(tab.id, entity);
    }
    let font = tab_font(ui_font.as_deref());
    for (position, id) in order.iter().enumerate() {
        if by_id.contains_key(id) {
            continue;
        }
        let label = tab_label(position, tab_name(&tabs.entries, *id));
        let tab = spawn_tab(&mut commands, strip, *id, label, &font);
        by_id.insert(*id, tab);
    }
    let mut ordered: Vec<Entity> = order
        .iter()
        .filter_map(|id| by_id.get(id).copied())
        .collect();
    ordered.extend(plus.iter());
    if strip_children.map(|children| &children[..]) != Some(&ordered[..]) {
        commands.entity(strip).replace_children(&ordered);
    }
}

/// Spawns one rounded tab, shorter than the bar and centred in it, with
/// its label and close button under `strip`, and returns it.
fn spawn_tab(
    commands: &mut Commands,
    strip: Entity,
    tab: TabId,
    label: String,
    font: &TextFont,
) -> Entity {
    /// The narrowest a tab gets before the strip scrolls, in logical px.
    const TAB_MIN_WIDTH_PX: f32 = 80.0;
    /// The widest a tab gets, in logical px.
    const TAB_MAX_WIDTH_PX: f32 = 240.0;

    let tab = commands
        .spawn((
            TabButton { id: tab },
            Node {
                flex_grow: 1.0,
                flex_basis: Val::Px(TAB_MAX_WIDTH_PX),
                min_width: Val::Px(TAB_MIN_WIDTH_PX),
                max_width: Val::Px(TAB_MAX_WIDTH_PX),
                height: Val::Px(20.0),
                align_self: AlignSelf::Center,
                padding: UiRect::horizontal(Val::Px(12.0)),
                column_gap: Val::Px(8.0),
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(6.0)),
                ..default()
            },
            ChildOf(strip),
        ))
        .observe(on_tab_click)
        .observe(on_tab_enter)
        .observe(on_tab_leave)
        .id();
    commands
        .spawn((
            TabLabel,
            Node {
                flex_grow: 1.0,
                min_width: Val::Px(0.0),
                overflow: Overflow::clip_x(),
                ..default()
            },
            ChildOf(tab),
        ))
        .with_child((
            TabLabelText,
            Text::new(label),
            font.clone(),
            TextColor(INACTIVE_TEXT),
            TextLayout::no_wrap(),
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
    tab
}

/// The name of tab `id` among `entries`; `None` when it has none or
/// is not listed.
fn tab_name(entries: &[TabEntry], id: TabId) -> Option<&str> {
    entries
        .iter()
        .find(|entry| entry.id == id)
        .and_then(|entry| entry.name.as_deref())
}

/// The text entities inside the `TabLabel` among a tab's `parts`.
fn label_texts<'a>(
    parts: &'a Children,
    label_boxes: &'a Query<&Children, With<TabLabel>>,
) -> impl Iterator<Item = Entity> + 'a {
    parts
        .iter()
        .filter_map(|part| label_boxes.get(part).ok())
        .flat_map(|texts| texts.iter())
}

/// Fills the displayed tab, and a hovered one more faintly, colors each
/// label and close button for whether its tab is displayed, and shows the
/// close button on the displayed tab and on a hovered one.
fn style_tabs(
    mut buttons: Query<(&TabButton, &mut BackgroundColor, &Children, Has<TabHovered>)>,
    mut labels: Query<&mut TextColor, With<TabLabelText>>,
    mut closes: Query<(&mut Node, &mut TextColor), (With<TabClose>, Without<TabLabelText>)>,
    tabs: Res<CurrentTabs>,
    label_boxes: Query<&Children, With<TabLabel>>,
) {
    for (tab, mut background, parts, hovered) in &mut buttons {
        let displayed = tabs.active == Some(tab.id);
        let (fill, text, close_text) = if displayed {
            (ACTIVE_BG, ACTIVE_TEXT, ACTIVE_CLOSE)
        } else if hovered {
            (HOVER_BG, INACTIVE_TEXT, INACTIVE_TEXT)
        } else {
            (Color::NONE, INACTIVE_TEXT, INACTIVE_TEXT)
        };
        let close = if displayed || hovered {
            Display::Flex
        } else {
            Display::None
        };
        background.set_if_neq(BackgroundColor(fill));
        for label in label_texts(parts, &label_boxes) {
            if let Ok(mut color) = labels.get_mut(label) {
                color.set_if_neq(TextColor(text));
            }
        }
        for part in parts {
            let Ok((mut node, mut color)) = closes.get_mut(*part) else {
                continue;
            };
            if node.display != close {
                node.display = close;
            }
            color.set_if_neq(TextColor(close_text));
        }
    }
}

/// A primary click on a tab requests that its tab be displayed, and
/// a primary double-click starts renaming it instead; a click that ends a
/// drag of that tab, or lands on a tab being renamed, does nothing.
fn on_tab_click(
    ev: On<Pointer<Click>>,
    mut commands: Commands,
    buttons: Query<&TabButton>,
    drag: Res<TabDrag>,
    rename: Res<TabRename>,
) {
    if ev.button != PointerButton::Primary {
        return;
    }
    let Ok(tab) = buttons.get(ev.entity) else {
        return;
    };
    if drag.is_dragging(tab.id) || rename.tab() == Some(tab.id) {
        return;
    }
    if ev.count == 2 {
        commands.trigger(StartTabRename { tab: Some(tab.id) });
        return;
    }
    commands.trigger(RequestTabAction {
        action: TabAction::Select(TabTarget::Id(tab.id)),
    });
}

/// A primary click on a close button requests its tab's close, unless the
/// click ends a drag of that tab; no click on it reaches the tab.
fn on_close_click(
    mut ev: On<Pointer<Click>>,
    mut commands: Commands,
    parents: Query<&ChildOf>,
    buttons: Query<&TabButton>,
    drag: Res<TabDrag>,
) {
    ev.propagate(false);
    if ev.button != PointerButton::Primary {
        return;
    }
    let Some(tab) = parents
        .get(ev.entity)
        .ok()
        .and_then(|parent| buttons.get(parent.parent()).ok())
    else {
        return;
    };
    if drag.is_dragging(tab.id) {
        return;
    }
    commands.trigger(RequestTabAction {
        action: TabAction::Close(CloseTarget::Id(tab.id)),
    });
}

/// A primary click on the plus button requests a new tab.
fn on_new_tab_click(ev: On<Pointer<Click>>, mut commands: Commands) {
    if ev.button == PointerButton::Primary {
        commands.trigger(PaneSpawnRequest { at: NewPaneAt::Tab });
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

/// Scrolls the strip so the displayed tab is fully visible
/// when the displayed tab changes; a list change that keeps the same
/// tab displayed leaves the scroll position alone.
fn scroll_active_tab_into_view(
    mut commands: Commands,
    mut scrolled_to: Local<Option<TabId>>,
    tabs: Res<CurrentTabs>,
    buttons: Query<(Entity, &TabButton)>,
) {
    let Some(active) = tabs.active else {
        return;
    };
    if *scrolled_to == Some(active) {
        return;
    }
    if let Some((entity, _)) = buttons.iter().find(|(_, tab)| tab.id == active) {
        *scrolled_to = Some(active);
        commands.trigger(ScrollIntoView { entity });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::NormalizedRenderTarget;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input_focus::InputFocus;
    use bevy::math::Affine2;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::{Location, PointerId};
    use bevy::text::{EditableText, TextCursorStyle};
    use bevy::ui::CalculatedClip;
    use bevy::ui::update::update_clipping_system;
    use bevy_orzmux::prelude::PendingTabMove;
    use orzmux::prelude::CommandSeq;
    use orzmux::prelude::PaneId;
    use std::fmt::Debug;
    use std::time::Duration;

    /// A request the tab bar sent, in the order it was sent.
    #[derive(Debug, PartialEq)]
    enum Sent {
        Tab(TabAction),
        Spawn(NewPaneAt),
    }

    #[derive(Resource, Default)]
    struct SentRequests(Vec<Sent>);

    fn app_with_tab_bar() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TabBarPlugin))
            .init_resource::<CurrentTabs>()
            .init_resource::<InputFocus>();
        app.world_mut().spawn((Node::default(), UiRoot));
        app
    }

    fn set_tabs(app: &mut App, ids: &[u32], active: u32) {
        let mut tabs = app.world_mut().resource_mut::<CurrentTabs>();
        tabs.entries = ids
            .iter()
            .map(|id| TabEntry {
                id: TabId(*id),
                name: None,
                active_pane: PaneId(*id),
            })
            .collect();
        tabs.active = Some(TabId(active));
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
    fn listed_tabs(app: &mut App) -> Vec<(TabId, String)> {
        let children = strip_children(app);
        let world = app.world();
        children
            .iter()
            .filter_map(|child| {
                let tab = world.get::<TabButton>(*child)?;
                let label = world
                    .get::<Children>(label_box_in(world, *child))?
                    .iter()
                    .find_map(|text| world.get::<Text>(text))?;
                Some((tab.id, label.0.clone()))
            })
            .collect()
    }

    /// The tab's direct child that carries `TabLabel`.
    fn label_box_in(world: &World, tab: Entity) -> Entity {
        world
            .get::<Children>(tab)
            .expect("the tab has parts")
            .iter()
            .find(|part| world.get::<TabLabel>(*part).is_some())
            .expect("the label box is a direct child of the tab")
    }

    fn only_child(app: &App, parent: Entity) -> Entity {
        let children = app.world().get::<Children>(parent).expect("has children");
        assert_eq!(children.len(), 1, "exactly one child");
        children[0]
    }

    fn place(app: &mut App, entity: Entity, center: Vec2, size: Vec2) {
        app.world_mut().entity_mut(entity).insert((
            ComputedNode { size, ..default() },
            UiGlobalTransform::from(Affine2::from_translation(center)),
        ));
    }

    fn tab_of(app: &mut App, tab: u32) -> Entity {
        let world = app.world_mut();
        world
            .query::<(Entity, &TabButton)>()
            .iter(world)
            .find(|(_, button)| button.id == TabId(tab))
            .map(|(entity, _)| entity)
            .expect("the tab has a tab")
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

    fn new_tab_button(app: &mut App) -> Entity {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<NewTabButton>>()
            .single(world)
            .expect("exactly one new-tab button")
    }

    fn record_requests(app: &mut App) {
        app.init_resource::<SentRequests>()
            .add_observer(|ev: On<RequestTabAction>, mut sent: ResMut<SentRequests>| {
                sent.0.push(Sent::Tab(ev.action.clone()));
            })
            .add_observer(|ev: On<PaneSpawnRequest>, mut sent: ResMut<SentRequests>| {
                sent.0.push(Sent::Spawn(ev.at));
            });
    }

    #[derive(Resource, Default)]
    struct ScrolledIntoView(Vec<Entity>);

    fn record_scrolls_into_view(app: &mut App) {
        app.init_resource::<ScrolledIntoView>().add_observer(
            |ev: On<ScrollIntoView>, mut scrolled: ResMut<ScrolledIntoView>| {
                scrolled.0.push(ev.entity);
            },
        );
    }

    fn strip_of(app: &mut App) -> Entity {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<TabStrip>>()
            .single(world)
            .expect("exactly one tab strip")
    }

    /// Lays out a 1000 px strip at the window's left edge with 100 px tabs
    /// for `ids` from `TAB_STRIP_LEFT_PADDING_PX`, `TAB_GAP_PX` apart.
    fn lay_out_strip(app: &mut App, ids: &[u32]) {
        let strip = strip_of(app);
        place(app, strip, Vec2::new(500.0, 14.0), Vec2::new(1000.0, 28.0));
        for (slot, id) in ids.iter().enumerate() {
            let tab = tab_of(app, *id);
            let center = Vec2::new(
                TAB_STRIP_LEFT_PADDING_PX + 50.0 + (100.0 + TAB_GAP_PX) * slot as f32,
                14.0,
            );
            place(app, tab, center, Vec2::new(100.0, 20.0));
        }
    }

    fn pointer_at<E: Debug + Clone + Reflect>(entity: Entity, x: f32, event: E) -> Pointer<E> {
        Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::None {
                    width: 800,
                    height: 600,
                },
                position: Vec2::new(x, 14.0),
            },
            event,
            entity,
        )
    }

    fn drag_start(entity: Entity, x: f32) -> Pointer<DragStart> {
        let hit = HitData::new(Entity::PLACEHOLDER, 0.0, None, None);
        let event = DragStart {
            button: PointerButton::Primary,
            hit,
        };
        pointer_at(entity, x, event)
    }

    fn drag_to(entity: Entity, x: f32, distance: f32) -> Pointer<Drag> {
        let event = Drag {
            button: PointerButton::Primary,
            distance: Vec2::new(distance, 0.0),
            delta: Vec2::new(distance, 0.0),
        };
        pointer_at(entity, x, event)
    }

    fn drag_end(entity: Entity, x: f32, distance: f32) -> Pointer<DragEnd> {
        let event = DragEnd {
            button: PointerButton::Primary,
            distance: Vec2::new(distance, 0.0),
        };
        pointer_at(entity, x, event)
    }

    fn expected_tabs(tabs: &[(u32, &str)]) -> Vec<(TabId, String)> {
        tabs.iter()
            .map(|(id, label)| (TabId(*id), (*label).to_string()))
            .collect()
    }

    fn click(entity: Entity) -> Pointer<Click> {
        clicks(entity, 1)
    }

    fn clicks(entity: Entity, count: u8) -> Pointer<Click> {
        let event = Click {
            button: PointerButton::Primary,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            duration: Duration::ZERO,
            count,
        };
        pointer_at(entity, 0.0, event)
    }

    /// Asserts that an unnamed tab shows `Tab n` by position and a
    /// named tab shows its name.
    ///
    /// Case: the user renamed the second of three tabs.
    #[test]
    fn labels_follow_the_position_unless_named() {
        assert_eq!(tab_label(0, None), "Tab 1");
        assert_eq!(tab_label(1, Some("logs")), "logs");
        assert_eq!(tab_label(2, None), "Tab 3");
    }

    /// Asserts that one button per tab exists in display order, and
    /// that a closed tab is removed.
    ///
    /// Case: the user opens three tabs, then closes the middle one.
    #[test]
    fn tabs_follow_the_tab_list() {
        let mut app = app_with_tab_bar();
        set_tabs(&mut app, &[1, 2, 3], 1);
        app.update();
        app.update();

        assert_eq!(
            listed_tabs(&mut app),
            vec![
                (TabId(1), "Tab 1".to_string()),
                (TabId(2), "Tab 2".to_string()),
                (TabId(3), "Tab 3".to_string()),
            ]
        );
        let plus = new_tab_button(&mut app);
        assert_eq!(strip_children(&mut app).last(), Some(&plus));

        set_tabs(&mut app, &[1, 3], 1);
        app.update();

        assert_eq!(
            listed_tabs(&mut app),
            vec![
                (TabId(1), "Tab 1".to_string()),
                (TabId(3), "Tab 2".to_string()),
            ]
        );
        assert_eq!(strip_children(&mut app).last(), Some(&plus));
        let world = app.world_mut();
        assert_eq!(
            world.query::<&TabButton>().iter(world).count(),
            2,
            "the closed tab is despawned"
        );
    }

    /// Asserts that clicking a tab requests its selection, clicking its
    /// close button requests its close without also selecting it, and the
    /// plus button requests a new tab.
    ///
    /// Case: the user clicks the second tab, then the first tab's ×, then +.
    #[test]
    fn clicks_become_tab_requests() {
        let mut app = app_with_tab_bar();
        record_requests(&mut app);
        set_tabs(&mut app, &[1, 2], 1);
        app.update();
        app.update();

        let second = tab_of(&mut app, 2);
        let first = tab_of(&mut app, 1);
        let close_first = close_button_of(&mut app, first);
        let plus = new_tab_button(&mut app);
        app.world_mut().trigger(click(second));
        app.world_mut().trigger(click(close_first));
        app.world_mut().trigger(click(plus));
        app.update();

        assert_eq!(
            app.world().resource::<SentRequests>().0,
            vec![
                Sent::Tab(TabAction::Select(TabTarget::Id(TabId(2)))),
                Sent::Tab(TabAction::Close(CloseTarget::Id(TabId(1)))),
                Sent::Spawn(NewPaneAt::Tab),
            ]
        );
    }

    /// Asserts that the displayed tab is filled, a hovered tab is filled
    /// more faintly, and the others are left unfilled, and that the × shows
    /// on the displayed and the hovered tab only, lighter on the displayed
    /// one.
    ///
    /// Case: three tabs are open with the first on screen, and the
    /// pointer rests on the second tab.
    #[test]
    fn the_displayed_and_hovered_tabs_are_filled_with_a_close() {
        let mut app = app_with_tab_bar();
        set_tabs(&mut app, &[1, 2, 3], 1);
        app.update();
        app.update();
        let tabs = [1, 2, 3].map(|id| tab_of(&mut app, id));
        let closes = tabs.map(|tab| close_button_of(&mut app, tab));
        app.world_mut().entity_mut(tabs[1]).insert(TabHovered);
        app.update();

        let world = app.world();
        let fills = tabs.map(|tab| world.get::<BackgroundColor>(tab).map(|fill| fill.0));
        assert_eq!(fills, [Some(ACTIVE_BG), Some(HOVER_BG), Some(Color::NONE)]);
        let shown = closes.map(|close| {
            let display = world.get::<Node>(close).map(|node| node.display);
            let color = world.get::<TextColor>(close).map(|color| color.0);
            display.zip(color)
        });
        assert_eq!(
            shown,
            [
                Some((Display::Flex, ACTIVE_CLOSE)),
                Some((Display::Flex, INACTIVE_TEXT)),
                Some((Display::None, INACTIVE_TEXT)),
            ]
        );
    }

    /// Asserts that a tab's × turns back to the dim text color once the tab
    /// is no longer displayed, and that the newly displayed tab's × turns
    /// light.
    ///
    /// Case: the pointer rests on the first tab while the user switches to
    /// the second tab with a key binding.
    #[test]
    fn the_close_follows_the_displayed_tab() {
        let mut app = app_with_tab_bar();
        set_tabs(&mut app, &[1, 2], 1);
        app.update();
        app.update();
        let tabs = [1, 2].map(|id| tab_of(&mut app, id));
        let closes = tabs.map(|tab| close_button_of(&mut app, tab));
        app.world_mut().entity_mut(tabs[0]).insert(TabHovered);
        set_tabs(&mut app, &[1, 2], 2);
        app.update();

        let world = app.world();
        let colors = closes.map(|close| world.get::<TextColor>(close).map(|color| color.0));
        assert_eq!(colors, [Some(INACTIVE_TEXT), Some(ACTIVE_CLOSE)]);
    }

    /// Asserts that a double-click on a tab replaces its label with a
    /// focused rename field holding the label, in the label's place, that
    /// draws a caret in the label's text color and a highlight behind its
    /// selection, and selects nothing; a further click on that tab is
    /// ignored.
    ///
    /// Case: the user double-clicks the second of two tabs, then clicks
    /// inside the rename field that appears.
    #[test]
    fn a_double_click_opens_the_rename_field_in_place_of_the_label() {
        let mut app = app_with_tab_bar();
        record_requests(&mut app);
        set_tabs(&mut app, &[1, 2], 2);
        app.update();
        app.update();
        let second = tab_of(&mut app, 2);
        let label_box = label_box_in(app.world(), second);

        app.world_mut().trigger(clicks(second, 2));
        app.update();
        let field = app
            .world()
            .get::<Children>(second)
            .expect("the tab has parts")[0];
        app.world_mut().trigger(clicks(field, 3));
        app.update();

        let world = app.world();
        let parts = world.get::<Children>(second).expect("the tab has parts");
        assert_eq!(parts[0], field);
        assert_eq!(
            world
                .get::<EditableText>(field)
                .map(|text| text.value().to_string()),
            Some("Tab 2".to_string())
        );
        assert_eq!(parts[1], label_box);
        assert_eq!(
            world.get::<Node>(label_box).map(|node| node.display),
            Some(Display::None)
        );
        assert_eq!(world.resource::<InputFocus>().get(), Some(field));
        let cursor = world
            .get::<TextCursorStyle>(field)
            .expect("the rename field draws a caret and a selection");
        assert_eq!(cursor.color, ACTIVE_TEXT);
        assert_ne!(cursor.selection_color, Color::NONE);
        assert_eq!(world.resource::<TabRename>().tab(), Some(TabId(2)));
        assert!(world.resource::<SentRequests>().0.is_empty());
    }

    /// Asserts that a drag begun inside a rename field follows no tab.
    ///
    /// Case: the user drags across the text in the rename field to select
    /// part of the name.
    #[test]
    fn a_drag_inside_the_rename_field_moves_no_tab() {
        let mut app = app_with_tab_bar();
        set_tabs(&mut app, &[1, 2], 2);
        app.update();
        app.update();
        lay_out_strip(&mut app, &[1, 2]);
        let second = tab_of(&mut app, 2);
        app.world_mut().trigger(clicks(second, 2));
        app.update();
        let field = app
            .world()
            .get::<Children>(second)
            .expect("the tab has parts")[0];

        app.world_mut().trigger(drag_start(field, 130.0));
        app.world_mut().trigger(drag_to(field, 190.0, 60.0));
        app.update();

        assert!(!app.world().resource::<TabDrag>().tracks(TabId(2)));
    }

    /// Asserts that a tab's label text is clipped horizontally at the edges
    /// of its label box, a direct child of the tab, rather than at the
    /// strip's edges.
    ///
    /// Case: a tab has shrunk to its minimum width, and its label is wider
    /// than the room left beside the ×.
    #[test]
    fn a_long_label_is_clipped_at_its_label_box() {
        let mut app = app_with_tab_bar();
        set_tabs(&mut app, &[1], 1);
        app.update();
        app.update();
        let tab = tab_of(&mut app, 1);
        let label_box = label_box_in(app.world(), tab);
        let text = only_child(&app, label_box);
        let strip = strip_of(&mut app);
        place(
            &mut app,
            strip,
            Vec2::new(500.0, 14.0),
            Vec2::new(1000.0, 28.0),
        );
        place(
            &mut app,
            label_box,
            Vec2::new(32.5, 14.0),
            Vec2::new(41.0, 14.0),
        );
        place(&mut app, text, Vec2::new(47.0, 14.0), Vec2::new(70.0, 14.0));

        app.world_mut()
            .run_system_once(update_clipping_system)
            .expect("the clipping system runs");

        let clip = app
            .world()
            .get::<CalculatedClip>(text)
            .expect("the label text is clipped")
            .clip;
        assert_eq!((clip.min.x, clip.max.x), (12.0, 53.0));
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

    /// Asserts that the preview moves the dragged tab to its target slot
    /// and leaves the others in order.
    ///
    /// Case: the user drags the first of three tabs onto the last slot.
    #[test]
    fn the_preview_moves_the_dragged_tab() {
        let entries: Vec<TabEntry> = [1, 2, 3]
            .into_iter()
            .map(|id| TabEntry {
                id: TabId(id),
                name: None,
                active_pane: PaneId(id),
            })
            .collect();
        assert_eq!(
            tab_order(&entries, Some((TabId(1), 2))),
            vec![TabId(2), TabId(3), TabId(1)]
        );
        assert_eq!(
            tab_order(&entries, None),
            vec![TabId(1), TabId(2), TabId(3)]
        );
    }

    /// Asserts that a tab dragged past the threshold previews the new order
    /// with renumbered labels while it follows the pointer, that its drop
    /// sends one move and no selection, and that the dropped order stays
    /// until the backend answers, even with a no-op.
    ///
    /// Case: the user drags the first of three tabs onto the third slot and
    /// releases it there, and the backend answers without moving anything.
    #[test]
    fn a_dropped_tab_sends_its_move_and_selects_nothing() {
        let mut app = app_with_tab_bar();
        record_requests(&mut app);
        app.init_resource::<PendingTabMove>();
        set_tabs(&mut app, &[1, 2, 3], 1);
        app.update();
        app.update();
        lay_out_strip(&mut app, &[1, 2, 3]);
        let first = tab_of(&mut app, 1);
        let previewed = expected_tabs(&[(2, "Tab 1"), (3, "Tab 2"), (1, "Tab 3")]);

        app.world_mut().trigger(drag_start(first, 50.0));
        app.world_mut().trigger(drag_to(first, 280.0, 230.0));
        app.update();

        assert_eq!(listed_tabs(&mut app), previewed);
        assert_eq!(
            app.world()
                .get::<UiTransform>(first)
                .expect("a tab has a transform")
                .translation
                .x,
            Val::Px(230.0 - 2.0 * (100.0 + TAB_GAP_PX))
        );

        app.world_mut().trigger(click(first));
        app.world_mut().trigger(drag_end(first, 280.0, 230.0));
        app.update();

        assert_eq!(
            app.world().resource::<SentRequests>().0,
            vec![Sent::Tab(TabAction::Move {
                tab: TabId(1),
                index: 2,
            })]
        );
        assert_eq!(
            app.world().get::<UiTransform>(first),
            Some(&UiTransform::default())
        );
        app.world_mut().resource_mut::<PendingTabMove>().0 = Some(CommandSeq(7));
        app.update();
        assert_eq!(listed_tabs(&mut app), previewed);

        app.world_mut().resource_mut::<PendingTabMove>().0 = None;
        app.update();

        assert_eq!(
            listed_tabs(&mut app),
            expected_tabs(&[(1, "Tab 1"), (2, "Tab 2"), (3, "Tab 3")])
        );
    }

    /// Asserts that a tab released over the gap after another tab lands in
    /// that tab's slot, counting the strip's leading padding.
    ///
    /// Case: the user drags the first of three tabs and releases it just
    /// past the right edge of the second one.
    #[test]
    fn a_drop_in_the_gap_after_a_tab_lands_in_its_slot() {
        let mut app = app_with_tab_bar();
        record_requests(&mut app);
        set_tabs(&mut app, &[1, 2, 3], 1);
        app.update();
        app.update();
        lay_out_strip(&mut app, &[1, 2, 3]);
        let first = tab_of(&mut app, 1);

        app.world_mut().trigger(drag_start(first, 56.0));
        app.world_mut().trigger(drag_to(first, 212.0, 156.0));
        app.world_mut().trigger(drag_end(first, 212.0, 156.0));
        app.update();

        assert_eq!(
            app.world().resource::<SentRequests>().0,
            vec![Sent::Tab(TabAction::Move {
                tab: TabId(1),
                index: 1,
            })]
        );
    }

    /// Asserts that a tab dragged past the threshold stacks above every
    /// other child of the strip, and drops back to their stacking at the
    /// end of the drag; a press under the threshold is not raised.
    ///
    /// Case: the user presses the first of three tabs, nudges it, then
    /// drags it over the third tab and releases it there.
    #[test]
    fn a_dragged_tab_stacks_above_its_neighbours_until_released() {
        let mut app = app_with_tab_bar();
        set_tabs(&mut app, &[1, 2, 3], 1);
        app.update();
        app.update();
        lay_out_strip(&mut app, &[1, 2, 3]);
        let first = tab_of(&mut app, 1);
        let z_of = |app: &App, entity: Entity| {
            *app.world()
                .get::<ZIndex>(entity)
                .expect("a strip child has a z-index")
        };

        app.world_mut().trigger(drag_start(first, 50.0));
        app.world_mut().trigger(drag_to(first, 52.0, 2.0));
        app.update();

        assert_eq!(z_of(&app, first), ZIndex::default());

        app.world_mut().trigger(drag_to(first, 190.0, 140.0));
        app.update();

        let raised = z_of(&app, first);
        for sibling in strip_children(&mut app) {
            if sibling != first {
                assert!(
                    raised.0 > z_of(&app, sibling).0,
                    "the dragged tab stacks above {sibling:?}"
                );
            }
        }

        app.world_mut().trigger(drag_end(first, 190.0, 140.0));
        app.update();

        assert_eq!(z_of(&app, first), ZIndex::default());
    }

    /// Asserts that a press moved less than the threshold stays a click
    /// that selects the tab and sends no move.
    ///
    /// Case: the user's hand shakes by 2 px while clicking the second tab.
    #[test]
    fn a_nudged_tab_is_selected_and_not_moved() {
        let mut app = app_with_tab_bar();
        record_requests(&mut app);
        set_tabs(&mut app, &[1, 2, 3], 1);
        app.update();
        app.update();
        lay_out_strip(&mut app, &[1, 2, 3]);
        let second = tab_of(&mut app, 2);

        app.world_mut().trigger(drag_start(second, 198.0));
        app.world_mut().trigger(drag_to(second, 200.0, 2.0));
        app.world_mut().trigger(click(second));
        app.world_mut().trigger(drag_end(second, 200.0, 2.0));
        app.update();

        assert_eq!(
            app.world().resource::<SentRequests>().0,
            vec![Sent::Tab(TabAction::Select(TabTarget::Id(TabId(2))))]
        );
        assert_eq!(
            listed_tabs(&mut app),
            expected_tabs(&[(1, "Tab 1"), (2, "Tab 2"), (3, "Tab 3")])
        );
    }

    /// Asserts that a drag begun on a tab's close button moves the
    /// tab and closes nothing.
    ///
    /// Case: the user presses the × of the displayed first tab, drags the
    /// tab onto the third slot, and releases it over the ×.
    #[test]
    fn a_drag_from_the_close_button_closes_nothing() {
        let mut app = app_with_tab_bar();
        record_requests(&mut app);
        set_tabs(&mut app, &[1, 2, 3], 1);
        app.update();
        app.update();
        lay_out_strip(&mut app, &[1, 2, 3]);
        let first = tab_of(&mut app, 1);
        let close_first = close_button_of(&mut app, first);

        app.world_mut().trigger(drag_start(close_first, 90.0));
        app.world_mut().trigger(drag_to(close_first, 290.0, 200.0));
        app.world_mut().trigger(click(close_first));
        app.world_mut().trigger(drag_end(close_first, 290.0, 200.0));
        app.update();

        assert_eq!(
            app.world().resource::<SentRequests>().0,
            vec![Sent::Tab(TabAction::Move {
                tab: TabId(1),
                index: 2,
            })]
        );
    }

    /// Asserts that a tab is scrolled into view when the displayed
    /// tab changes, and not when the list changes while the same
    /// tab stays displayed.
    ///
    /// Case: with the second of three tabs on screen, the user closes
    /// the hidden first one from its tab and then switches to the third.
    #[test]
    fn only_a_display_change_scrolls_a_tab_into_view() {
        let mut app = app_with_tab_bar();
        record_scrolls_into_view(&mut app);
        app.update();
        set_tabs(&mut app, &[1, 2, 3], 2);
        app.update();
        let second = tab_of(&mut app, 2);
        let third = tab_of(&mut app, 3);

        set_tabs(&mut app, &[2, 3], 2);
        app.update();
        set_tabs(&mut app, &[2, 3], 3);
        app.update();

        assert_eq!(
            app.world().resource::<ScrolledIntoView>().0,
            vec![second, third]
        );
    }
}
