//! Tab bar Bevy UI builders for a Pane. `build_tab` spawns one tab (a
//! `Button` + its `Text` child) into a pane's tab-bar row; the chrome layer
//! (`crate::ui::chrome`) owns the row Node and calls `build_tab` per surface.
//! `tab_colors` computes the (background, indicator, text) color triple for a
//! single tab.

use crate::theme;
use crate::ui::palette;
use crate::ui::TabButton;
use bevy::color::Color;
use bevy::prelude::*;
use bevy::text::{LineBreak, TextLayout};
use bevy::ui::{AlignItems, BorderRadius, JustifyContent, Overflow, UiRect, Val};

/// Color triple for one tab.
struct TabColors {
    bg: Color,
    indicator: Color,
    text: Color,
}

/// Compute the (background, top-indicator, text) color triple for one tab.
/// Indicator is `palette::ACCENT` only when the tab and its pane are both
/// active; an active tab inside an inactive pane gets `palette::BORDER`;
/// inactive tabs get `Color::NONE`.
fn tab_colors(is_active: bool, is_active_pane: bool) -> TabColors {
    let bg = if is_active {
        palette::TAB_ACTIVE_BG
    } else {
        Color::NONE
    };
    let indicator = match (is_active, is_active_pane) {
        (true, true) => palette::ACCENT,
        (true, false) => palette::BORDER,
        (false, _) => Color::NONE,
    };
    let text = if is_active {
        palette::FOREGROUND
    } else {
        palette::MUTED
    };
    TabColors {
        bg,
        indicator,
        text,
    }
}

/// A single tab's display data, derived from ECS components by the caller.
pub(crate) struct TabEntry {
    /// Surface entity this tab selects. Attached to the tab Node as
    /// `TabButton.surface` so `drive_tab_clicks` can focus it.
    pub entity: Entity,
    /// Display name of the surface.
    pub name: String,
    /// Whether this surface is the pane's `ActiveSurface`.
    pub is_active: bool,
}

/// Spawn one tab (a `Button` Node + its `Text` child) as a child of the
/// pane's tab-bar row `parent`. `is_active_pane` drives the indicator accent
/// (accent vs border).
pub(crate) fn build_tab(
    commands: &mut Commands,
    parent: Entity,
    pane: Entity,
    tab: &TabEntry,
    is_active_pane: bool,
    ui_font: &Handle<Font>,
) {
    let colors = tab_colors(tab.is_active, is_active_pane);

    let tab_entity = commands
        .spawn((
            Name::new("Tab"),
            Button,
            TabButton {
                pane,
                surface: tab.entity,
            },
            Node {
                padding: UiRect::axes(Val::Px(theme::TAB_PADDING_X_PX), Val::Px(4.0)),
                border: UiRect {
                    top: Val::Px(theme::TAB_INDICATOR_PX),
                    right: Val::Px(theme::BORDER_PX),
                    left: Val::Px(theme::BORDER_PX),
                    bottom: Val::ZERO,
                },
                border_radius: BorderRadius {
                    top_left: Val::Px(theme::TAB_BORDER_RADIUS_PX),
                    top_right: Val::Px(theme::TAB_BORDER_RADIUS_PX),
                    bottom_left: Val::Px(0.0),
                    bottom_right: Val::Px(0.0),
                },
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                max_width: Val::Px(theme::TAB_MAX_WIDTH_PX),
                overflow: Overflow::clip_x(),
                ..default()
            },
            BackgroundColor(colors.bg),
            BorderColor {
                top: colors.indicator,
                left: theme::BORDER,
                right: theme::BORDER,
                bottom: Color::NONE,
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(tab.name.clone()),
        TextColor(colors.text),
        TextFont {
            font: ui_font.clone(),
            font_size: 12.0,
            ..default()
        },
        TextLayout {
            linebreak: LineBreak::NoWrap,
            ..default()
        },
        ChildOf(tab_entity),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_colors_active_in_active_pane_uses_accent_indicator() {
        let c = tab_colors(true, true);
        assert_eq!(c.bg, palette::TAB_ACTIVE_BG);
        assert_eq!(c.indicator, palette::ACCENT);
        assert_eq!(c.text, palette::FOREGROUND);
    }

    #[test]
    fn tab_colors_active_in_inactive_pane_uses_border_indicator() {
        let c = tab_colors(true, false);
        assert_eq!(c.bg, palette::TAB_ACTIVE_BG);
        assert_eq!(c.indicator, palette::BORDER);
    }

    #[test]
    fn tab_colors_inactive_is_fully_transparent() {
        let c = tab_colors(false, true);
        assert_eq!(c.bg, Color::NONE);
        assert_eq!(c.indicator, Color::NONE);
        assert_eq!(c.text, palette::MUTED);
    }
}
