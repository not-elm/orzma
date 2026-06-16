//! ozmux Bevy GUI entry point.

mod bootstrap;
mod clipboard;
mod configs;
mod control_plane;
mod font;
mod inline_webview;
mod input;
mod multiplexer;
mod osc_webview;
mod system_set;
mod theme;
mod tmux_copy_mode;
mod tmux_input;
mod tmux_mouse;
mod tmux_pane_hit;
mod tmux_picker;
mod tmux_render;
mod ui;
mod webview_render;

use crate::control_plane::OzmuxControlPlanePlugin;
use crate::inline_webview::OzmuxInlineWebviewPlugin;
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::osc_webview::OzmuxOscWebviewPlugin;
use crate::webview_render::{OzmuxWebviewRenderPlugin, cef_plugin};
use bevy::prelude::*;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
use multiplexer::log::OzmuxLayoutLogPlugin;
use ozma_tty_engine::TerminalHandlePlugin;
use ozma_tty_renderer::TerminalRendererPlugin;
use ozmux_multiplexer::MultiplexerPlugin;
use ozmux_tmux::TmuxSessionPlugin;
use ozmux_webview_host::DynAssetRegistry;
use tmux_copy_mode::OzmuxTmuxCopyModePlugin;
use tmux_input::OzmuxTmuxInputPlugin;
use tmux_mouse::OzmuxTmuxMousePlugin;
use tmux_picker::OzmuxTmuxPickerPlugin;
use tmux_render::OzmuxTmuxRenderPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::tmux_dialog::TmuxDialogPlugin;
use ui::tmux_divider_handle::OzmuxTmuxDividerHandlePlugin;
use ui::tmux_pane_focus::OzmuxTmuxPaneFocusPlugin;
use ui::tmux_window_bar::OzmuxTmuxWindowBarPlugin;
use ui::{
    OzmuxUiPlugin, confirm_prompt::ConfirmPromptPlugin, copy_mode::CopyModePlugin,
    copy_mode_indicator::CopyModeIndicatorPlugin, copy_search::CopyPromptPlugin,
};

fn main() {
    let dyn_registry = DynAssetRegistry::default();
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "ozmux".to_string(),
                    ime_enabled: true,
                    ..default()
                }),
                ..default()
            }),
            cef_plugin(dyn_registry.clone()),
        ))
        .add_plugins((
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            MultiplexerPlugin,
            TmuxSessionPlugin,
            OzmuxTmuxPickerPlugin,
            OzmuxConfigsPlugin,
            FontBridgePlugin,
            OzmuxLayoutLogPlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
            OzmuxWebviewRenderPlugin,
            CopyModePlugin,
            CopyModeIndicatorPlugin,
        ))
        .add_plugins(CopyPromptPlugin)
        .add_plugins(ConfirmPromptPlugin)
        .add_plugins(TmuxDialogPlugin)
        .add_plugins(OzmuxTmuxRenderPlugin)
        .add_plugins(OzmuxTmuxInputPlugin)
        .add_plugins(OzmuxTmuxWindowBarPlugin)
        .add_plugins(OzmuxTmuxPaneFocusPlugin)
        .add_plugins(OzmuxTmuxCopyModePlugin)
        .add_plugins(OzmuxTmuxMousePlugin)
        .add_plugins(OzmuxTmuxDividerHandlePlugin)
        .add_plugins((
            HyperlinkInputPlugin,
            ImePlugin,
            ImeOverlayPlugin,
            OzmuxOscWebviewPlugin,
            OzmuxInlineWebviewPlugin,
            OzmuxControlPlanePlugin::new(dyn_registry),
        ))
        .run();
}
