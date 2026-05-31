//! Extension Node process lifecycle: spawn at daemon startup, graceful
//! shutdown via stdin EOF with SIGKILL fallback after a 500ms grace period.

use crate::{
    error::ExtensionResult, handle::package_json::PackageJson, registry::ExtensionRegistry,
    runtime::RuntimeRoot,
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::process::{Child, Command};

mod package_json;

/// Owns every Node extension child process spawned by the daemon and is
/// responsible for tearing them down on daemon shutdown via [`Self::shutdown`].
pub struct ExtensionHandles {
    children: Vec<Child>,
}

impl ExtensionHandles {
    /// Discovers extensions under `OZMUX_EXTENSION_ROOT`, spawns each as a
    /// Node child process, and returns a handle owning them. Returns an
    /// empty handle if the env var is unset or empty.
    pub fn load(runtime: &RuntimeRoot, registry: ExtensionRegistry) -> ExtensionResult<Self> {
        const OZMUX_EXTENSION_ROOT: &str = "OZMUX_EXTENSION_ROOT";
        let root = match std::env::var(OZMUX_EXTENSION_ROOT) {
            Ok(s) if !s.is_empty() => s,
            _ => {
                tracing::info!(
                    "{OZMUX_EXTENSION_ROOT} is not set; daemon will run without extensions"
                );
                return Ok(Self { children: vec![] });
            }
        };
        let mut children = Vec::new();
        tracing::info!("extension root dir={root}");
        for entry in std::fs::read_dir(root)?.filter_map(|r| r.ok()) {
            let extension_dir = entry.path();
            match load_package_json(&extension_dir).and_then(|package| {
                node_handle(package.clone(), &extension_dir, runtime, &registry)
            }) {
                Ok(h) => children.push(h),
                Err(e) => tracing::error!("{e}"),
            }
        }
        Ok(Self { children })
    }

    /// Gracefully shuts down every spawned extension. For each child:
    /// `tokio::process::Child::wait()` drops the stdin handle first, which
    /// closes the pipe and triggers the Node-side EOF cleanup handler. If
    /// the child does not exit within `SHUTDOWN_TIMEOUT`, it is SIGKILLed
    /// and reaped. Idempotent: leaves the internal vector empty.
    pub async fn shutdown(&mut self) {
        for child in std::mem::take(&mut self.children) {
            shutdown_one(child).await;
        }
    }
}

const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

async fn shutdown_one(mut child: Child) {
    match tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait()).await {
        Ok(Ok(_status)) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "extension wait failed"),
        Err(_) => {
            let _ = child.kill().await;
        }
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
    registry: &ExtensionRegistry,
) -> ExtensionResult<Child> {
    let bin_dir = runtime.bin_dir().join(&package.name);
    let sock_path = runtime.sock_dir().join(format!("{}.sock", package.name));
    let handlers_sock_path = runtime
        .sock_dir()
        .join(format!("{}.handlers.sock", package.name));

    std::fs::create_dir_all(&bin_dir)?;
    #[cfg(unix)]
    {
        std::fs::set_permissions(&bin_dir, std::fs::Permissions::from_mode(0o700))?;
    }

    registry.register(&package.name, extension_dir);
    registry.set_handlers_sock_path(&package.name, &handlers_sock_path);

    let spawn_result = Command::new("node")
        .arg(&package.main)
        .current_dir(extension_dir)
        .env("EXTENSION_NAME", &package.name)
        .env("OZMUX_BIN_DIR", &bin_dir)
        .env("OZMUX_SOCK_PATH", &sock_path)
        .env("OZMUX_HANDLERS_SOCK_PATH", &handlers_sock_path)
        .env("OZMUX_EXTENSION_HOST_URL", "http://127.0.0.1:3200")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();

    match spawn_result {
        Ok(child) => {
            tracing::info!("spawn extension process: {}", package.name);
            Ok(child)
        }
        Err(e) => {
            registry.unregister(&package.name);
            Err(e.into())
        }
    }
}
