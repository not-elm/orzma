use crate::{
    error::{ExtensionError, ExtensionResult},
    handle::package_json::PackageJson,
    host::resolve_socket_path,
};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
};

mod package_json;

pub struct ExtensionHandles {
    _node_handles: Vec<Child>,
}

impl ExtensionHandles {
    pub fn load() -> ExtensionResult<Self> {
        const OZMUX_EXTENSION_ROOT: &str = "OZMUX_EXTENSION_ROOT";
        let root = std::env::var(OZMUX_EXTENSION_ROOT)
            .map_err(|_| ExtensionError::MissingEnv(OZMUX_EXTENSION_ROOT.to_string()))?;
        let mut handles = Vec::new();
        for entry in std::fs::read_dir(root)?.filter_map(|r| r.ok()) {
            let Some(package) = load_package_json(&entry.path()) else {
                continue;
            };
            match node_handle(package) {
                Ok(h) => handles.push(h),
                Err(e) => {
                    tracing::error!("{e}");
                }
            }
        }
        Ok(Self {
            _node_handles: handles,
        })
    }
}

fn load_package_json(extension_dir: &Path) -> Option<PackageJson> {
    let buff = std::fs::read_to_string(extension_dir.join("package.json")).ok()?;
    serde_json::from_str(&buff).ok()
}

fn node_handle(package: PackageJson) -> std::io::Result<Child> {
    Command::new("node")
        .arg(package.main)
        .env("EXTENSION_NAME", package.name)
        .env("EXTENSION_HOST_SOCKET_PATH", resolve_socket_path())
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}
