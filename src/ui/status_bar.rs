//! Status bar Bevy UI builder. Spawns one Row Node containing one chip
//! per Session, sorted by Entity id (bit-stable across frames within a
//! run). The attached session's chip gets the accent color.

use crate::theme;
use crate::theme::UI_FONT_SIZE;
use crate::ui::StructuralNode;
use crate::ui::palette;
use crate::ui::status_bar_sync::StatusBarRoot;
use bevy::prelude::*;
use bevy::ui::{AlignItems, AlignSelf, FlexDirection, UiRect, Val};

/// Spawn the status bar as a child of `parent`. `sessions` is a slice of
/// `(entity, name, is_attached)` tuples, one per Session entity, sorted
/// by the caller. `attached_entity` is the Entity with `AttachedSession`,
/// used to accent the matching chip.
pub(crate) fn build_status_bar(
    commands: &mut Commands,
    parent: Entity,
    sessions: &[(Entity, String, bool)],
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
            StructuralNode,
            StatusBarRoot,
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Node {
            width: Val::Px(theme::ELEMENT_PADDING_PX),
            ..default()
        },
        StructuralNode,
        ChildOf(bar),
    ));
    commands.spawn((
        Node {
            width: Val::Px(theme::BORDER_PX),
            align_self: AlignSelf::Stretch,
            ..default()
        },
        BackgroundColor(palette::BORDER),
        StructuralNode,
        ChildOf(bar),
    ));
    commands.spawn((
        Node {
            width: Val::Px(theme::ELEMENT_PADDING_PX),
            ..default()
        },
        StructuralNode,
        ChildOf(bar),
    ));

    build_session_chips(commands, bar, sessions, ui_font);
}

fn build_session_chips(
    commands: &mut Commands,
    bar: Entity,
    sessions: &[(Entity, String, bool)],
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
            StructuralNode,
            ChildOf(bar),
        ))
        .id();

    for (_entity, name, is_attached) in sessions {
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
            StructuralNode,
            ChildOf(container),
        ));
    }
}
