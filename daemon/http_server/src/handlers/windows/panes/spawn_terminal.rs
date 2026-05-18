//! Shared terminal PTY spawn used by `split.rs` and `add_to_pane.rs`.
//!
//! Rollback is the caller's responsibility because the rollback shape
//! differs (split removes a pane subtree; add_activity removes a single
//! activity from a pane).

use crate::AppState;
use crate::error::HttpResult;
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};
use ozmux_terminal::SpawnOptions;
use std::path::Path;

/// Spawn the PTY for a freshly-added terminal Activity in `pid` of `wid`.
/// `cwd` is the initial working directory for the spawned shell; `None`
/// inherits the daemon process's CWD. Returns the underlying
/// `TerminalError` on spawn failure; the caller owns the rollback.
pub(crate) async fn spawn_terminal_pty(
    state: &AppState,
    wid: &WindowId,
    pid: &PaneId,
    aid: &ActivityId,
    cwd: Option<&Path>,
) -> HttpResult<()> {
    let session_id = super::session_owning_window(state, wid).await;
    state
        .terminal
        .spawn(
            pid.clone(),
            aid.clone(),
            SpawnOptions {
                cols: 80,
                rows: 24,
                shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                cwd: cwd.map(|p| p.to_string_lossy().into_owned()),
                window_id: Some(wid.clone()),
                session_id,
            },
        )
        .await?;
    Ok(())
}
