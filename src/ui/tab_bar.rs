//! The workspace tab bar across the top of the window: its node, its
//! height, and the tabs it lists.

use crate::ui::UiRoot;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowScaleFactorChanged};

/// The tab bar's height in logical px before rounding to physical pixels.
pub(crate) const TAB_BAR_HEIGHT_PX: f32 = 28.0;

/// The workspace tab bar's root node, the first child of `UiRoot`.
#[derive(Component)]
pub(crate) struct TabBar;

/// Spawns the tab bar and keeps its height on whole physical pixels.
pub(crate) struct TabBarPlugin;

impl Plugin for TabBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<WindowScaleFactorChanged>().add_systems(
            Update,
            (
                ensure_tab_bar.run_if(not(any_with_component::<TabBar>)),
                size_tab_bar.run_if(on_message::<WindowScaleFactorChanged>),
            ),
        );
    }
}

/// The tab bar's height in whole physical pixels at `scale_factor`.
pub(crate) fn tab_bar_height_phys(scale_factor: f32) -> u32 {
    (TAB_BAR_HEIGHT_PX * scale_factor).round().max(0.0) as u32
}

/// The bar's background.
const BAR_BG: Color = Color::srgb_u8(0x14, 0x15, 0x18);
/// The line under the bar and between inactive tabs.
const BAR_LINE: Color = Color::srgb_u8(0x59, 0x59, 0x66);

/// The 1 px line along the bar's bottom edge. It is the bar's first child,
/// so the tabs paint over it and the displayed tab's opaque background
/// hides it under that tab.
#[derive(Component)]
struct TabBarLine;

/// The bar's height in logical px that lands on whole physical pixels.
fn tab_bar_height_logical(scale_factor: f32) -> f32 {
    tab_bar_height_phys(scale_factor) as f32 / scale_factor.max(f32::EPSILON)
}

/// Spawns the bar as the first child of `UiRoot`, above the shell surface.
fn ensure_tab_bar(
    mut commands: Commands,
    ui_root: Query<Entity, With<UiRoot>>,
    window: Query<&Window, With<PrimaryWindow>>,
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
    commands.entity(ui_root).insert_children(0, &[bar]);
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

#[cfg(test)]
mod tests {
    use super::*;

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
