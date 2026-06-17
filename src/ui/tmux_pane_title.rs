//! Per-pane title bar: `PaneTitleBar` marker and the plugin that keeps it in sync.

use crate::theme;
use bevy::prelude::*;
use ozma_tty_engine::TerminalTitle;
use ozmux_tmux::{ActivePane, TmuxPane};

/// Marker on the title-bar child entity that sits at the top of each `TmuxPane`
/// container.
#[derive(Component)]
pub(crate) struct PaneTitleBar;

/// Keeps each pane's title bar text and color in sync with `TerminalTitle` and
/// `ActivePane` state.
pub(crate) struct OzmuxTmuxPaneTitlePlugin;

impl Plugin for OzmuxTmuxPaneTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (sync_pane_title_text, sync_pane_title_active),
        );
    }
}

/// Updates the `Text` grandchild of each `PaneTitleBar` when `TerminalTitle` changes.
fn sync_pane_title_text(
    mut texts: Query<&mut Text>,
    changed: Query<(&TerminalTitle, &Children), (With<TmuxPane>, Changed<TerminalTitle>)>,
    bars: Query<&Children, With<PaneTitleBar>>,
) {
    for (title, pane_children) in changed.iter() {
        let Some(bar) = pane_children.iter().find(|c| bars.contains(*c)) else {
            continue;
        };
        let Ok(bar_children) = bars.get(bar) else {
            continue;
        };
        for text_entity in bar_children.iter() {
            if let Ok(mut text) = texts.get_mut(text_entity) {
                let s = title.0.as_deref().unwrap_or("");
                *text = Text::new(s);
            }
        }
    }
}

/// Recolors each pane's title bar: `TAB_BAR_BG` + accent outline for the active
/// pane, `PANEL` + transparent outline otherwise. Write-guarded to avoid
/// triggering a UI relayout on every frame.
fn sync_pane_title_active(
    mut bars: Query<(&mut BackgroundColor, &mut Outline), With<PaneTitleBar>>,
    panes: Query<(Has<ActivePane>, &Children), With<TmuxPane>>,
) {
    for (active, children) in panes.iter() {
        for child in children.iter() {
            let Ok((mut bg, mut outline)) = bars.get_mut(child) else {
                continue;
            };
            let want_bg = if active {
                BackgroundColor(theme::TAB_BAR_BG)
            } else {
                BackgroundColor(theme::PANEL)
            };
            let want_outline_color = if active { theme::ACCENT } else { Color::NONE };
            if *bg != want_bg {
                *bg = want_bg;
            }
            if outline.color != want_outline_color {
                outline.color = want_outline_color;
            }
        }
    }
}
