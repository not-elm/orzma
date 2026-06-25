//! Propagates a UTF-8 `LC_CTYPE` into the adopted tmux server's environment on
//! the attach edge, so panes spawned afterward run their shells in a UTF-8
//! locale.
//!
//! Without this, a tmux server that was started without a UTF-8 locale (a
//! pre-existing server, or a launchd-launched `.app` whose env was stripped)
//! gives every pane shell the C/POSIX locale. zsh's line editor then re-renders
//! each typed multibyte glyph using its per-byte C-locale notation — an orphaned
//! UTF-8 lead byte (which renders as the replacement character) followed by
//! `<0083>`-style placeholders for the continuation bytes — which ozmux
//! faithfully displays as garbage. ozmux's own outgoing bytes are correct UTF-8;
//! the corruption is purely in zsh's echo of typed wide characters.
//!
//! The fix is a global `set-environment -g LC_CTYPE <utf8>`: tmux merges its
//! global environment into every pane process it spawns *after* the set. It
//! cannot retroactively fix a pane that forked earlier (including the adopted
//! control-mode pane), so an already-running shell must be restarted (or have
//! `LC_CTYPE` exported) to pick up the locale.
//!
//! Only `LC_CTYPE` is set — the one category zsh's wide-character echo depends
//! on, and the one `ensure_utf8_locale_env` already normalizes for ozmux's own
//! control client. A non-UTF-8 `LC_ALL` deliberately exported
//! into the adopted server's environment outranks `LC_CTYPE` and is left
//! untouched (overriding it would clobber a deliberate UTF-8 `LC_ALL` and other
//! locale categories), so that rarer case is not repaired by this.

use crate::utf8_ctype_for_panes;
use bevy::prelude::*;
use ozmux_tmux::{SetEnvironmentGlobal, TmuxClient, TmuxClientMut};

/// Registers the attach-edge `LC_CTYPE` propagation system.
pub(crate) struct TmuxLocalePlugin;

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

#[cfg(test)]
mod tests {
    use super::*;

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
