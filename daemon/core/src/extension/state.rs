use crate::{
    error::OzmuxResult,
    extension::manifest::{CommandName, CommandScriptPath},
};
use std::collections::HashMap;

pub struct ExtensionRegistory {
    commands: HashMap<CommandName, CommandScriptPath>,
}

impl ExtensionRegistory {
    pub async fn load() -> OzmuxResult<Self> {
        todo!("ExtensionRegistry::load implementation deferred to subsequent PR")
    }

    pub fn commands(&self) -> &HashMap<CommandName, CommandScriptPath> {
        &self.commands
    }
}
