//! Status bar Bevy UI builder. Spawns one Row Node containing the session
//! name + one chip per linked window. The active window's chip gets
//! `palette::ACCENT` background.

use crate::theme;
use crate::theme::UI_FONT_SIZE;
use crate::ui::StructuralNode;
use crate::ui::palette;
use bevy::prelude::*;
use bevy::ui::{AlignItems, AlignSelf, FlexDirection, UiRect, Val};
use ozmux_multiplexer::{Session, Window, WindowId};
use std::collections::HashMap;

/// Spawn the status bar (session name + window chips) as a child of `parent`.
pub(crate) fn build_status_bar(
    commands: &mut Commands,
    parent: Entity,
    session: &Session,
    active_wid: &WindowId,
    windows: &HashMap<WindowId, Window>,
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
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(&session.name),
        TextColor(palette::FOREGROUND),
        TextFont {
            font: ui_font.clone(),
            font_size: UI_FONT_SIZE,
            ..default()
        },
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

    build_window_are(commands, bar, session, active_wid, windows, ui_font);
}

fn build_window_are(
    commands: &mut Commands,
    bar: Entity,
    session: &Session,
    active_wid: &WindowId,
    windows: &HashMap<WindowId, Window>,
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
    for wid in &session.linked_windows {
        let label = windows
            .get(wid)
            .map(|w| w.name.clone())
            .unwrap_or_else(|| wid.to_string());
        let font_color = if wid == active_wid {
            palette::COPY_MODE_INDICATOR_BG
        } else {
            palette::FOREGROUND
        };

        commands.spawn((
            Text::new(label),
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
