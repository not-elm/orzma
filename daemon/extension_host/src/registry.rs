use crate::error::ExtensionHostResult;
use crate::manifest::{CommandName, CommandScriptPath};
use std::collections::HashMap;

pub struct ExtensionRegistry {
    commands: HashMap<CommandName, CommandScriptPath>,
}

impl ExtensionRegistry {
    /// Load extensions from `OZMUX_EXTENSION_DIR`.
    ///
    /// TODO: implementation deferred to a follow-up PR. The pre-split
    /// version had unresolved compile errors (`extension_root` vs
    /// `extension_dir`, missing return). Type signature is preserved.
    pub async fn load() -> ExtensionHostResult<Self> {
        todo!("ExtensionRegistry::load implementation deferred to subsequent PR")
    }

    pub fn commands(&self) -> &HashMap<CommandName, CommandScriptPath> {
        &self.commands
    }
}
