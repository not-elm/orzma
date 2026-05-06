use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::define_string_new_type;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionManifest {
    pub commands: HashMap<CommandName, CommandScriptPath>,
}

define_string_new_type!(CommandName);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandScriptPath(pub PathBuf);
