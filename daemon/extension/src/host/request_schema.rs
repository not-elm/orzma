use ozmux_session::{activity::ActivityId, pane::PaneId};
use serde::{Deserialize, Serialize};

use crate::manifest::CommandName;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ExtensionIpcRequest {
    ExecuteCommand {
        pane: PaneId,
        activity: ActivityId,
        command: CommandName,
        argv: Vec<String>,
    },
}
