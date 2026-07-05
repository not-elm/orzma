//! Propagates a UTF-8 `LC_CTYPE` into the adopted tmux server's environment on
//! the attach edge, so panes spawned afterward run their shells in a UTF-8
//! locale.
//!
//! Without this, a tmux server that was started without a UTF-8 locale (a
//! pre-existing server, or a launchd-launched `.app` whose env was stripped)
//! gives every pane shell the C/POSIX locale. zsh's line editor then re-renders
//! each typed multibyte glyph using its per-byte C-locale notation — an orphaned
//! UTF-8 lead byte (which renders as the replacement character) followed by
//! `<0083>`-style placeholders for the continuation bytes — which orzma
//! faithfully displays as garbage. orzma's own outgoing bytes are correct UTF-8;
//! the corruption is purely in zsh's echo of typed wide characters.
//!
//! The fix is a global `set-environment -g LC_CTYPE <utf8>`: tmux merges its
//! global environment into every pane process it spawns *after* the set. It
//! cannot retroactively fix a pane that forked earlier (including the adopted
//! control-mode pane), so an already-running shell must be restarted (or have
//! `LC_CTYPE` exported) to pick up the locale.
//!
//! Only `LC_CTYPE` is set — the one category zsh's wide-character echo depends
//! on, and the one `ensure_utf8_locale_env` already normalizes for orzma's own
//! control client. A non-UTF-8 `LC_ALL` deliberately exported
//! into the adopted server's environment outranks `LC_CTYPE` and is left
//! untouched (overriding it would clobber a deliberate UTF-8 `LC_ALL` and other
//! locale categories), so that rarer case is not repaired by this.

use crate::{UTF8_CTYPE_FALLBACK, is_utf8_locale};
use bevy::prelude::*;
use orzma_tmux::{SetEnvironmentGlobal, TmuxClient, TmuxClientMut};

/// Registers the attach-edge `LC_CTYPE` propagation system.
pub(super) struct TmuxLocalePlugin;

impl Plugin for TmuxLocalePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, propagate_utf8_ctype.run_if(tmux_client_added));
    }
}

/// Run condition: true on the frame a [`TmuxClient`] is newly adopted.
fn tmux_client_added(added: Query<(), Added<TmuxClient>>) -> bool {
    !added.is_empty()
}

/// Sends `set-environment -g LC_CTYPE <utf8>` over the adopted connection so
/// panes spawned after attach inherit a UTF-8 locale.
///
/// Runs exactly once per attach (`Added<TmuxClient>`). The value is resolved by
/// [`utf8_ctype_for_panes`], which is guaranteed UTF-8 on macOS.
fn propagate_utf8_ctype(mut client: TmuxClientMut<'_, '_>) {
    let ctype = utf8_ctype_for_panes();
    if let Err(error) = client.send(SetEnvironmentGlobal {
        key: "LC_CTYPE",
        value: &ctype,
    }) {
        tracing::warn!(?error, %ctype, "failed to set tmux global LC_CTYPE for new panes");
    }
}

/// The UTF-8 `LC_CTYPE` orzma advertises to tmux panes spawned after attach, so
/// their shells render typed wide characters correctly instead of as `<00xx>`
/// placeholders (see this module's docs for the full failure mode).
///
/// Resolves to the first UTF-8 locale among `LC_ALL` / `LC_CTYPE` / `LANG`,
/// skipping any non-UTF-8 entries, falling back to [`UTF8_CTYPE_FALLBACK`].
///
/// This deliberately differs from `utf8_locale_fallback` (in `src/main.rs`):
/// that one honors tmux's first-non-empty-wins precedence to decide *whether*
/// orzma's own process needs the fallback, so a leading non-UTF-8 value (e.g.
/// `LC_ALL=C`) makes it return the fallback. Here we instead want a concrete
/// UTF-8 *value* to advertise to panes, so a non-UTF-8 entry is skipped rather
/// than allowed to win. Because `ensure_utf8_locale_env` runs first, on macOS
/// this always resolves to a UTF-8 value even when the inherited environment
/// selected the C/POSIX locale.
fn utf8_ctype_for_panes() -> String {
    resolve_utf8_ctype(
        std::env::var("LC_ALL").ok().as_deref(),
        std::env::var("LC_CTYPE").ok().as_deref(),
        std::env::var("LANG").ok().as_deref(),
    )
}

/// Pure resolver behind [`utf8_ctype_for_panes`]: the first non-empty UTF-8
/// locale among the three values, or [`UTF8_CTYPE_FALLBACK`].
fn resolve_utf8_ctype(lc_all: Option<&str>, lc_ctype: Option<&str>, lang: Option<&str>) -> String {
    [lc_all, lc_ctype, lang]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty() && is_utf8_locale(value))
        .map(str::to_string)
        .unwrap_or_else(|| UTF8_CTYPE_FALLBACK.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_ctype_falls_back_when_no_utf8_locale() {
        assert_eq!(resolve_utf8_ctype(None, None, None), "en_US.UTF-8");
        assert_eq!(
            resolve_utf8_ctype(Some("C"), Some("C"), Some("C")),
            "en_US.UTF-8"
        );
        assert_eq!(
            resolve_utf8_ctype(Some(""), Some(""), Some("POSIX")),
            "en_US.UTF-8"
        );
    }

    #[test]
    fn resolve_ctype_keeps_first_utf8_locale() {
        // The user's own UTF-8 locale is preserved rather than overwritten with
        // the en_US fallback, honoring tmux's LC_ALL > LC_CTYPE > LANG order.
        assert_eq!(
            resolve_utf8_ctype(None, Some("ja_JP.UTF-8"), None),
            "ja_JP.UTF-8"
        );
        assert_eq!(
            resolve_utf8_ctype(Some("en_US.UTF-8"), Some("C"), None),
            "en_US.UTF-8"
        );
        assert_eq!(
            resolve_utf8_ctype(Some("C"), None, Some("ja_JP.UTF-8")),
            "ja_JP.UTF-8"
        );
    }

    #[test]
    fn propagate_sends_global_utf8_lc_ctype() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        app.add_systems(Update, propagate_utf8_ctype.run_if(tmux_client_added));
        app.update();

        let out = app
            .world_mut()
            .get_mut::<TmuxClient>(gateway)
            .unwrap()
            .take_outgoing();
        let command = String::from_utf8(out).expect("utf-8 command");
        assert!(
            command.starts_with("set-environment -g LC_CTYPE "),
            "attach must push a global LC_CTYPE: {command:?}"
        );
        let upper = command.to_ascii_uppercase();
        assert!(
            upper.contains("UTF-8") || upper.contains("UTF8"),
            "the advertised LC_CTYPE must be a UTF-8 locale: {command:?}"
        );
    }

    #[test]
    fn propagate_runs_once_per_attach_not_every_frame() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        app.add_systems(Update, propagate_utf8_ctype.run_if(tmux_client_added));

        app.update();
        let _ = app
            .world_mut()
            .get_mut::<TmuxClient>(gateway)
            .unwrap()
            .take_outgoing();

        // A second frame with no fresh `Added<TmuxClient>` edge must not resend.
        app.update();
        let out = app
            .world_mut()
            .get_mut::<TmuxClient>(gateway)
            .unwrap()
            .take_outgoing();
        assert!(
            out.is_empty(),
            "LC_CTYPE is set only on the attach edge, not every frame: {out:?}"
        );
    }
}
