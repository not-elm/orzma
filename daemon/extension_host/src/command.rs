//! IPC command DTOs received over the extension-host socket.

use ozmux_session::pane::PaneId;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct CreateActivity {
    pub pane: PaneId,
    pub view_path: String,
}
