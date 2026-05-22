//! Status bar Bevy UI builder. Spawns one Row Node containing the session
//! name + one chip per linked window. The active window's chip gets
//! `palette::ACCENT` background.

use crate::theme;
use crate::ui::StructuralNode;
use crate::ui::palette;
use bevy::color::Color;
use bevy::prelude::*;
use bevy::ui::{AlignItems, FlexDirection, UiRect, Val};
use ozmux_multiplexer::{Session, Window, WindowId};
use std::collections::HashMap;

/// Spawn the status bar (session name + window chips) as a child of `parent`.
pub(crate) fn build_status_bar(
    commands: &mut Commands,
    parent: Entity,
    session: &Session,
    active_wid: &WindowId,
    windows: &HashMap<WindowId, Window>,
) {
    let bar = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(theme::ELEMENT_PADDING_PX), Val::Px(0.0)),
                ..default()
            },
            BackgroundColor(palette::PANEL),
            StructuralNode,
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(session.name.clone()),
        TextColor(palette::FOREGROUND),
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

    for wid in &session.linked_windows {
        let label = windows
            .get(wid)
            .map(|w| w.name.clone())
            .unwrap_or_else(|| wid.to_string());
        let bg = if wid == active_wid {
            palette::ACCENT
        } else {
            Color::NONE
        };

        let chip = commands
            .spawn((
                Node {
                    padding: UiRect::axes(Val::Px(theme::ELEMENT_PADDING_PX), Val::Px(0.0)),
                    ..default()
                },
                BackgroundColor(bg),
                StructuralNode,
                ChildOf(bar),
            ))
            .id();

        commands.spawn((
            Text::new(label),
            TextColor(palette::FOREGROUND),
            StructuralNode,
            ChildOf(chip),
        ));
    }
}
