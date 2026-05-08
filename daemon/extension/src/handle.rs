use crate::{
    error::ExtensionResult,
    handle::package_json::PackageJson,
    runtime::RuntimeRoot,
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
    pub fn load(runtime: &RuntimeRoot) -> ExtensionResult<Self> {
        const OZMUX_EXTENSION_ROOT: &str = "OZMUX_EXTENSION_ROOT";
        let root = match std::env::var(OZMUX_EXTENSION_ROOT) {
            Ok(s) if !s.is_empty() => s,
            _ => {
                tracing::info!(
                    "{OZMUX_EXTENSION_ROOT} is not set; daemon will run without extensions"
                );
                return Ok(Self {
                    _node_handles: vec![],
                });
            }
        };
        let mut handles = Vec::new();
        tracing::info!("extension root dir={root}");
        for entry in std::fs::read_dir(root)?.filter_map(|r| r.ok()) {
            let extension_dir = entry.path();
            match load_package_json(&extension_dir)
                .and_then(|package| node_handle(package.clone(), &extension_dir, runtime))
            {
                Ok(h) => handles.push(h),
                Err(e) => tracing::error!("{e}"),
            }
        }
        Ok(Self {
            _node_handles: handles,
        })
    }
}

fn load_package_json(extension_dir: &Path) -> ExtensionResult<PackageJson> {
    let buff = std::fs::read_to_string(extension_dir.join("package.json"))?;
    Ok(serde_json::from_str(&buff)?)
}

fn node_handle(
    package: PackageJson,
    extension_dir: &Path,
    runtime: &RuntimeRoot,
) -> ExtensionResult<Child> {
    let bin_dir = runtime.bin_dir().join(&package.name);
    let sock_path = runtime.sock_dir().join(format!("{}.sock", package.name));

    std::fs::create_dir_all(&bin_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin_dir, std::fs::Permissions::from_mode(0o700))?;
    }

    let child = Command::new("node")
        .arg(&package.main)
        .current_dir(extension_dir)
        .env("EXTENSION_NAME", &package.name)
        .env("OZMUX_BIN_DIR", &bin_dir)
        .env("OZMUX_SOCK_PATH", &sock_path)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    tracing::info!("spawn extension process: {}", package.name);
    Ok(child)
}
