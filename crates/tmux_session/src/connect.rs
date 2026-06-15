//! Opening a control-mode connection from a chosen [`AttachTarget`].

use crate::select::AttachTarget;
use tmux_control::{TmuxClient, TmuxResult, TmuxServer};

/// Opens a `tmux -CC` connection for `target`: attaches to the named
/// session, or starts a fresh one.
pub fn attach_or_create(server: &TmuxServer, target: &AttachTarget) -> TmuxResult<TmuxClient> {
    match target {
        AttachTarget::Attach(name) => server.attach(name),
        AttachTarget::CreateNew => server.new_session(),
    }
}
