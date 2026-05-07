use std::{collections::HashMap, path::Path};

// #[derive(Debug, Clone)]
// pub struct ExtensionCommands(HashMap<CommandName, CommandScriptPath>);
//
// impl ExtensionCommands {
//     /// Materialize the command set as PATH wrapper scripts in a fresh
//     /// temp directory. Each wrapper is named `@<command>` and execs
//     /// `node '<escaped_script>' "$@"`. Returns the `TempDir` so its
//     /// `Drop` tears down the directory.
//     // pub fn materialize_wrappers(&self) -> ExtensionResult<tempfile::TempDir> {
//     //     let dir = tempfile::TempDir::new()?;
//     //     for (name, script) in &self.0 {
//     //         let wrapper_path = dir.path().join(format!("@{name}"));
//     //         std::fs::write(&wrapper_path, Self::wrapper_body(script))?;
//     //         Self::set_executable(&wrapper_path)?;
//     //     }
//     //     Ok(dir)
//     // }
//
//     pub fn iter(&self) -> impl Iterator<Item = (&CommandName, &CommandScriptPath)> {
//         self.0.iter()
//     }
//
//     pub fn is_empty(&self) -> bool {
//         self.0.is_empty()
//     }
//
//     // fn wrapper_body(script: &CommandScriptPath) -> String {
//     //     format!(
//     //         "#!/bin/sh\nexec node {} \"$@\"\n",
//     //         sh_single_quote(&script.0.display().to_string()),
//     //     )
//     // }
//
//     fn set_executable(path: &std::path::Path) -> std::io::Result<()> {
//         use std::os::unix::fs::PermissionsExt;
//         let mut perm = std::fs::metadata(path)?.permissions();
//         perm.set_mode(0o755);
//         std::fs::set_permissions(path, perm)
//     }
// }
