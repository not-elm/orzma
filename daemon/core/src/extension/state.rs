use crate::{
    define_string_new_type,
    error::OzmuxResult,
    extension::manifest::{CommandName, CommandScriptPath, ExtensionManifest},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

pub struct ExtensionRegistory {
    commands: HashMap<CommandName, CommandScriptPath>,
}

impl ExtensionRegistory {
    pub async fn load() -> OzmuxResult<Self> {
        let mut commands = HashMap::default();
        let extension_root = std::env::var("OZMUX_EXTENSION_DIR")?;
        for manifest in std::fs::read_dir(extension_dir)?
            .filter_map(|r| r.ok())
            .filter_map(load_manifest)
        {
            commands.extend(manifest.commands);
        }
        Ok(Self { commands })
    }
}

fn load_manifest(extension_dir: &PathBuf) -> Option<ExtensionManifest> {
    let buff = std::fs::read_to_string(extension_dir.join("ozmux.json")).ok()?;
    serde_json::from_str(&buff).ok()
}
