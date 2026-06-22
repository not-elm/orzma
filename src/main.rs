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
    // NOTE: must run before App::new() spawns any thread — it writes process
    // env vars, which is unsound once other threads may read the environment.
    ensure_terminfo_env();

    let pre_configs = ozmux_configs::OzmuxConfigs::load().unwrap_or_default();
    // NOTE: start in AppMode::Tmux as a boot-dispatch state; dispatch_startup_mode
    // (registered on OnEnter(Tmux), gated to run once) routes to the real startup
    // mode. The initial state MUST be Tmux for that OnEnter(Tmux) hook to fire;
    // routing out of it (to Default when configured) is a queued NextState applied
    // at the first post-Startup StateTransition.
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

/// Fills `TERM`/`COLORTERM` with a portable default when the inherited `TERM`
/// is unset or empty, mirroring `alacritty_terminal::tty::setup_env`.
///
/// A child shell (a tmux pane or the native ozma PTY) whose `TERM` is empty
/// cannot load terminfo, so zsh's line editor (ZLE) — Backspace included —
/// silently breaks; a bundled `.app` launched from Finder inherits launchd's
/// empty `TERM`. `xterm-256color` is exactly Alacritty's fallback when the
/// `alacritty` terminfo is absent (it is, on stock macOS); `COLORTERM`
/// advertises the 24-bit color that entry omits. A usable inherited `TERM` is
/// left untouched, so terminal launches are unchanged.
///
/// # Invariants
///
/// Must be called before any thread is spawned (i.e. at the very top of
/// `main()`): it writes process environment variables, which is unsound once
/// another thread may read the environment concurrently.
fn ensure_terminfo_env() {
    let Some(term) = term_fallback(std::env::var("TERM").ok().as_deref()) else {
        return;
    };
    // SAFETY: the caller invokes this before App::new() spawns any task-pool
    // thread, so no other thread can read the environment concurrently with
    // these writes (see # Invariants).
    unsafe {
        std::env::set_var("TERM", term);
        std::env::set_var("COLORTERM", "truecolor");
    }
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
