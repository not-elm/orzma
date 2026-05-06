use crate::error::{ExtensionHostError, ExtensionHostResult};
use crate::manifest::{CommandName, CommandScriptPath, ExtensionManifest};
use std::{collections::HashMap, path::Path};

#[derive(Debug, Clone)]
pub struct ExtensionCommands(HashMap<CommandName, CommandScriptPath>);

impl ExtensionCommands {
    pub async fn load() -> ExtensionHostResult<Self> {
        let mut commands = HashMap::default();
        let extension_root = std::env::var("OZMUX_EXTENSION_DIR")?;
        for entry in std::fs::read_dir(&extension_root)?.filter_map(|r| r.ok()) {
            if let Some(manifest) = load_manifest(&entry.path()) {
                commands.extend(manifest.commands);
            }
        }
        Ok(Self(commands))
    }

    pub async fn execute(&self, command: &CommandName, argv: &[String]) -> ExtensionHostResult {
        let cmd_path = self
            .0
            .get(command)
            .ok_or_else(|| ExtensionHostError::CommandNotFound(command.clone()))?;
        tokio::process::Command::new("node")
            .arg(&cmd_path.0)
            .args(argv)
            .spawn()?;
        Ok(())
    }
}

fn load_manifest(extension_dir: &Path) -> Option<ExtensionManifest> {
    let buff = std::fs::read_to_string(extension_dir.join("ozmux.json")).ok()?;
    serde_json::from_str(&buff).ok()
}
