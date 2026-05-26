//! Status bar Bevy UI builder. Spawns one Row Node containing one chip
//! per Session, sorted by `SessionId` (numeric, since `SessionEntityId`
//! wraps the monotonic counter). The attached session's chip gets the
//! accent color.

use crate::theme;
use crate::theme::UI_FONT_SIZE;
use crate::ui::StructuralNode;
use crate::ui::palette;
use bevy::prelude::*;
use bevy::ui::{AlignItems, AlignSelf, FlexDirection, UiRect, Val};
use ozmux_multiplexer::{Session, SessionId};
use std::collections::HashMap;

/// Spawn the status bar as a child of `parent`. `sessions_by_id` is the
/// domain HashMap from `MultiplexerService`. `attached_sid` colors the
/// matching chip with the accent.
pub(crate) fn build_status_bar(
    commands: &mut Commands,
    parent: Entity,
    sessions_by_id: &HashMap<SessionId, Session>,
    attached_sid: Option<SessionId>,
) {
    let mut ordered: Vec<&SessionId> = sessions_by_id.keys().collect();
    ordered.sort();

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

    build_session_chips(commands, bar, &ordered, sessions_by_id, attached_sid);
}

fn build_session_chips(
    commands: &mut Commands,
    bar: Entity,
    ordered: &[&SessionId],
    sessions_by_id: &HashMap<SessionId, Session>,
    attached_sid: Option<SessionId>,
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

    for sid in ordered {
        let session = match sessions_by_id.get(sid) {
            Some(s) => s,
            None => continue,
        };
        let font_color = if Some(**sid) == attached_sid {
            palette::COPY_MODE_INDICATOR_BG
        } else {
            palette::FOREGROUND
        };

        commands.spawn((
            Text::new(session.name.clone()),
            TextColor(font_color),
            TextFont {
                font_size: UI_FONT_SIZE,
                ..default()
            },
            StructuralNode,
            ChildOf(container),
        ));
    }
}
