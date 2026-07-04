//! ozmux Bevy GUI entry point.

mod action;
mod app_mode;
mod bootstrap;
mod cef_profile;
mod clipboard;
mod configs;
mod font;
mod input;
mod render;
mod session;
mod surface;
mod system_set;
mod theme;
mod ui;
mod webview_pointer;
mod window_title;

use crate::action::ActionPlugin;
use crate::app_mode::AppModePlugin;
use crate::cef_profile::CefProfileDir;
use crate::clipboard::ClipboardPlugin;
use crate::input::focus::FocusSyncPlugin;
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::surface::SurfacePlugin;
use crate::window_title::WindowTitlePlugin;
use bevy::prelude::*;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxInputPlugin;
use input::ime::ImePlugin;
use ozma_tty_engine::TerminalHandlePlugin;
use ozma_tty_renderer::TerminalRendererPlugin;
use ozma_webview::{OzmaWebviewPlugin, cef_plugin};
use ozmux_webview_host::WebviewAssetRegistry;
use render::tmux::RenderPlugin;
use session::default::DefaultSessionPlugin;
use session::tmux::TmuxLifecyclePlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::{
    OzmuxUiPlugin, copy_mode::CopyModePlugin, copy_mode_indicator::CopyModeIndicatorPlugin,
    copy_search::CopyPromptPlugin,
};

fn main() {
    // NOTE: must run before App::new() spawns any thread — they write process
    // env vars, which is unsound once other threads may read the environment.
    ensure_terminfo_env();
    ensure_utf8_locale_env();

    let pre_configs = ozmux_configs::OzmuxConfigs::load().unwrap_or_default();
    let ozma_registry = WebviewAssetRegistry::default();
    let cef_profile = CefProfileDir::acquire().expect("create per-process CEF profile directory");
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(primary_window()),
                ..default()
            }),
            cef_plugin(ozma_registry.clone(), cef_profile.path()),
        ))
        .add_plugins((
            AppModePlugin,
            SurfacePlugin,
            DefaultSessionPlugin {
                shell: pre_configs.ozma.shell.clone(),
            },
            ClipboardPlugin,
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            RenderPlugin,
            TmuxLifecyclePlugin,
            ActionPlugin,
            OzmuxConfigsPlugin,
            FontBridgePlugin,
            OzmuxBootstrapPlugin,
            OzmuxInputPlugin,
            OzmuxUiPlugin,
        ))
        .add_plugins((
            OzmaWebviewPlugin {
                ozma_assets: ozma_registry,
            },
            CopyModePlugin,
            CopyModeIndicatorPlugin,
            CopyPromptPlugin,
            WindowTitlePlugin,
            FocusSyncPlugin,
            HyperlinkInputPlugin,
            ImePlugin,
            ImeOverlayPlugin,
        ))
        .run();
}

/// The primary window descriptor.
///
/// `ime_enabled` starts `false` deliberately: bevy_winit applies the IME state
/// to the OS window only on a live `false -> true` change of `Window::ime_enabled`
/// (`bevy_winit-0.18.1/src/system.rs:503-504`) and never at window creation, so
/// starting `true` would leave the OS IME un-armed. `ime_policy_system` flips it
/// to `true` on the first focused-surface tick, producing the arming transition.
fn primary_window() -> Window {
    Window {
        title: "ozmux".to_string(),
        ime_enabled: false,
        ..default()
    }
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

/// The UTF-8 `LC_CTYPE` ozmux installs when the inherited locale is not UTF-8.
/// Guaranteed present on macOS, the only platform [`ensure_utf8_locale_env`]
/// writes it on. Also the fallback advertised to tmux panes
/// (`crate::session::tmux::locale`).
pub(crate) const UTF8_CTYPE_FALLBACK: &str = "en_US.UTF-8";

/// Ensures `LC_CTYPE` advertises a UTF-8 locale when the inherited environment
/// selects none, so tmux treats ozmux's control client as UTF-8 capable.
///
/// tmux replaces every TAB (and other non-printable byte) in `display-message`
/// / `list-windows` format output with `_` when its effective `LC_CTYPE` is not
/// UTF-8 (the C/POSIX locale). ozmux's tab-separated format queries
/// (`LIST_WINDOWS_FORMAT`, `COPY_STATE_FORMAT`) then collapse into a single
/// unsplittable field — which silently freezes copy-mode cursor/scroll updates,
/// since `parse_copy_state` returns `None` and never refreshes the overlay. A
/// bundled `.app` launched from Finder inherits launchd's environment with no
/// `LANG`/`LC_*`, so it falls into the C locale; this restores a UTF-8
/// `LC_CTYPE`. A usable inherited UTF-8 locale is left untouched.
///
/// # Invariants
///
/// Must be called before any thread is spawned (same constraint as
/// [`ensure_terminfo_env`]): it writes a process environment variable.
fn ensure_utf8_locale_env() {
    if utf8_locale_fallback(
        std::env::var("LC_ALL").ok().as_deref(),
        std::env::var("LC_CTYPE").ok().as_deref(),
        std::env::var("LANG").ok().as_deref(),
    )
    .is_none()
    {
        return;
    }
    set_utf8_ctype_fallback();
}

/// Writes the `en_US.UTF-8` `LC_CTYPE` fallback. macOS is the only platform that
/// ships the bundled `.app` hitting launchd's stripped env; elsewhere the locale
/// may be absent and forcing it would fail `setlocale`, so this is a no-op there
/// (see the `#[cfg(not(...))]` sibling).
#[cfg(target_os = "macos")]
fn set_utf8_ctype_fallback() {
    // SAFETY: the caller (`ensure_utf8_locale_env`) runs before `App::new()`
    // spawns any task-pool thread, so no other thread can read the environment
    // concurrently with this write (see that fn's # Invariants).
    unsafe {
        std::env::set_var("LC_CTYPE", UTF8_CTYPE_FALLBACK);
    }
}

/// No-op: the UTF-8 `LC_CTYPE` fallback is only written on macOS.
#[cfg(not(target_os = "macos"))]
fn set_utf8_ctype_fallback() {}

/// The UTF-8 `LC_CTYPE` ozmux substitutes when the effective locale is not
/// UTF-8, or `None` to keep the inherited locale.
///
/// Mirrors tmux's own `LC_ALL` > `LC_CTYPE` > `LANG` resolution: the first
/// non-empty of the three decides the character type. Returns the fallback when
/// that value is absent or not UTF-8; otherwise `None`.
fn utf8_locale_fallback(
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> Option<&'static str> {
    let effective = [lc_all, lc_ctype, lang]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty());
    match effective {
        Some(value) if is_utf8_locale(value) => None,
        _ => Some(UTF8_CTYPE_FALLBACK),
    }
}

/// Returns whether a locale string selects the UTF-8 codeset (`…UTF-8`,
/// `…UTF8`, or the bare macOS `UTF-8`), case-insensitively.
pub(crate) fn is_utf8_locale(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    upper.contains("UTF-8") || upper.contains("UTF8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_window_starts_with_ime_disabled() {
        // NOTE: bevy_winit never applies `ime_enabled` at window creation; it
        // calls `set_ime_allowed` only on a live `false -> true` change. Starting
        // `true` means that transition never fires and the OS IME never arms.
        assert!(!primary_window().ime_enabled);
    }

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

    #[test]
    fn unset_locale_gets_utf8_fallback() {
        assert_eq!(utf8_locale_fallback(None, None, None), Some("en_US.UTF-8"));
    }

    #[test]
    fn empty_locale_gets_utf8_fallback() {
        assert_eq!(
            utf8_locale_fallback(Some(""), Some(""), Some("")),
            Some("en_US.UTF-8")
        );
    }

    #[test]
    fn non_utf8_locale_gets_fallback() {
        assert_eq!(
            utf8_locale_fallback(None, Some("C"), None),
            Some("en_US.UTF-8")
        );
        assert_eq!(
            utf8_locale_fallback(None, None, Some("POSIX")),
            Some("en_US.UTF-8")
        );
    }

    #[test]
    fn utf8_locale_is_preserved() {
        assert_eq!(utf8_locale_fallback(None, Some("en_US.UTF-8"), None), None);
        assert_eq!(utf8_locale_fallback(None, None, Some("ja_JP.UTF-8")), None);
        assert_eq!(utf8_locale_fallback(None, Some("UTF-8"), None), None);
        assert_eq!(utf8_locale_fallback(None, None, Some("en_US.utf8")), None);
    }

    #[test]
    fn lc_all_takes_precedence_over_lang() {
        // LC_ALL=C wins even when LANG is UTF-8 → fallback applies.
        assert_eq!(
            utf8_locale_fallback(Some("C"), None, Some("en_US.UTF-8")),
            Some("en_US.UTF-8")
        );
        // LC_ALL UTF-8 wins even when LANG is C → preserved.
        assert_eq!(
            utf8_locale_fallback(Some("en_US.UTF-8"), None, Some("C")),
            None
        );
    }
}
