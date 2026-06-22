//! ozmux Bevy GUI entry point.

mod app_mode;
mod bootstrap;
mod cef_profile;
mod configs;
mod default_input;
mod font;
mod input;
mod picker;
mod system_set;
mod theme;
mod tmux;
mod ui;
mod window_title;

use crate::app_mode::{AppMode, DefaultModePlugin};
use crate::cef_profile::CefProfileDir;
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::window_title::WindowTitlePlugin;
use bevy::prelude::*;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use default_input::DefaultHostInputPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
use input::option_as_alt::OptionAsAltPlugin;
use ozma_terminal::OzmaTerminalPlugin;
use ozma_tty_engine::TerminalHandlePlugin;
use ozma_tty_renderer::TerminalRendererPlugin;
use ozma_webview::{OzmaWebviewPlugin, cef_plugin};
use ozmux_configs::StartupMode;
use ozmux_webview_host::WebviewAssetRegistry;
use picker::OzmuxPickerPlugin;
use tmux::OzmuxTmuxPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::{
    OzmuxUiPlugin, confirm_prompt::ConfirmPromptPlugin, copy_mode::CopyModePlugin,
    copy_mode_indicator::CopyModeIndicatorPlugin, copy_search::CopyPromptPlugin,
    rename_prompt::RenamePromptPlugin,
};

fn main() {
    // Mirror alacritty_terminal::tty::setup_env: a child shell (a tmux pane or
    // the native ozma PTY) whose TERM is empty/unset cannot load terminfo, so
    // zsh's line editor (ZLE) — Backspace included — silently breaks. A bundled
    // .app launched from Finder inherits launchd's empty TERM, so fill a portable
    // default before any PTY child is spawned. `xterm-256color` is exactly what
    // Alacritty falls back to when the `alacritty` terminfo is absent (it is, on
    // stock macOS); `COLORTERM` advertises the 24-bit color that entry omits.
    if let Some(term) = term_fallback(std::env::var("TERM").ok().as_deref()) {
        // SAFETY: this runs at the very top of main(), before App::new() spawns
        // any task-pool threads, so no other thread can read the environment
        // concurrently with these writes.
        unsafe {
            std::env::set_var("TERM", term);
            std::env::set_var("COLORTERM", "truecolor");
        }
    }

    let pre_configs = ozmux_configs::OzmuxConfigs::load().unwrap_or_default();
    // NOTE: start in AppMode::Tmux as a boot-dispatch state; dispatch_startup_mode
    // (OnEnter(Tmux), gated to run once) routes to the real mode. Routing to Default
    // via a queued NextState — rather than booting straight into Default — defers
    // OnEnter(AppMode::Default) (spawn_terminal) to a post-Startup StateTransition, so
    // Startup deferred commands (e.g. init_atlas_image inserting AtlasImage) flush first.
    let initial_mode = match pre_configs.startup_mode {
        StartupMode::Default | StartupMode::Tmux | StartupMode::TmuxAutoAttach => AppMode::Tmux,
    };
    let ozma_registry = WebviewAssetRegistry::default();
    let cef_profile = CefProfileDir::acquire().expect("create per-process CEF profile directory");
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
            cef_plugin(ozma_registry.clone(), cef_profile.path()),
        ))
        .insert_state(initial_mode)
        .add_plugins((
            OzmaTerminalPlugin {
                config_shell: pre_configs.ozma.shell.clone(),
            },
            DefaultModePlugin,
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            OzmuxTmuxPlugin,
            OzmuxPickerPlugin,
            OzmuxConfigsPlugin,
            FontBridgePlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
            OzmaWebviewPlugin {
                osc_enabled: pre_configs.osc_webview.enabled,
                ozma_assets: ozma_registry,
            },
            CopyModePlugin,
            CopyModeIndicatorPlugin,
        ))
        .add_plugins(CopyPromptPlugin)
        .add_plugins(ConfirmPromptPlugin)
        .add_plugins(RenamePromptPlugin)
        .add_plugins(WindowTitlePlugin)
        .add_plugins((
            HyperlinkInputPlugin,
            ImePlugin,
            ImeOverlayPlugin,
            OptionAsAltPlugin,
            DefaultHostInputPlugin,
        ))
        .run();
}

/// The portable `TERM` ozmux substitutes when the inherited one cannot resolve
/// terminfo, or `None` to keep a usable inherited value. Returns the fallback
/// for an unset (`None`) or empty `TERM`; otherwise `None`.
fn term_fallback(current: Option<&str>) -> Option<&'static str> {
    match current {
        Some(term) if !term.is_empty() => None,
        _ => Some("xterm-256color"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_term_gets_fallback() {
        assert_eq!(term_fallback(None), Some("xterm-256color"));
    }

    #[test]
    fn empty_term_gets_fallback() {
        assert_eq!(term_fallback(Some("")), Some("xterm-256color"));
    }

    #[test]
    fn valid_term_is_preserved() {
        assert_eq!(term_fallback(Some("tmux-256color")), None);
        assert_eq!(term_fallback(Some("xterm-256color")), None);
        assert_eq!(term_fallback(Some("screen")), None);
    }
}
