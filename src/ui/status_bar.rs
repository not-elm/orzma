//! Status bar Bevy UI builder. Spawns one Row Node containing one chip
//! per Workspace, in the order provided by the caller (status_bar_sync
//! sorts by `WorkspaceCreatedAt` so chips appear in creation order with
//! the oldest leftmost). The attached workspace's chip gets the accent
//! color.

use crate::theme;
use crate::theme::UI_FONT_SIZE;
use crate::ui::palette;
use crate::ui::status_bar_sync::StatusBarRoot;
use bevy::prelude::*;
use bevy::ui::{AlignItems, AlignSelf, FlexDirection, UiRect, Val};

/// Spawn the status bar as a child of `parent`. `workspaces` is a slice of
/// `(entity, name, is_attached)` tuples, one per Workspace entity, sorted
/// by the caller. `attached_entity` is the Entity with `AttachedWorkspace`,
/// used to accent the matching chip.
pub(crate) fn build_status_bar(
    commands: &mut Commands,
    parent: Entity,
    workspaces: &[(Entity, String, bool)],
    ui_font: &Handle<Font>,
) {
    let bar = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(16.0), Val::Px(0.0)),
                ..default()
            },
            BackgroundColor(palette::PANEL),
            StatusBarRoot,
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Node {
            width: Val::Px(theme::ELEMENT_PADDING_PX),
            ..default()
        },
        ChildOf(bar),
    ));
    commands.spawn((
        Node {
            width: Val::Px(theme::BORDER_PX),
            align_self: AlignSelf::Stretch,
            ..default()
        },
        BackgroundColor(palette::BORDER),
        ChildOf(bar),
    ));
    commands.spawn((
        Node {
            width: Val::Px(theme::ELEMENT_PADDING_PX),
            ..default()
        },
        ChildOf(bar),
    ));

    build_workspace_chips(commands, bar, workspaces, ui_font);
}

fn build_workspace_chips(
    commands: &mut Commands,
    bar: Entity,
    workspaces: &[(Entity, String, bool)],
    ui_font: &Handle<Font>,
) {
    let container = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                padding: UiRect::ZERO,
                column_gap: Val::Px(8.0),
                ..default()
            },
            BackgroundColor(palette::PANEL),
            ChildOf(bar),
        ))
        .id();

    for (_entity, name, is_attached) in workspaces {
        let font_color = if *is_attached {
            palette::COPY_MODE_INDICATOR_BG
        } else {
            palette::FOREGROUND
        };

        commands.spawn((
            Text::new(name.clone()),
            TextColor(font_color),
            TextFont {
                font: ui_font.clone(),
                font_size: UI_FONT_SIZE,
                ..default()
            },
            ChildOf(container),
        ));
    }
}
