use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use ozmux_macros::NewType;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionManifest {
    pub commands: HashMap<CommandName, CommandScriptPath>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
pub struct CommandName(String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandScriptPath(pub PathBuf);
