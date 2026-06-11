//! Spawns the single Node host process (the `@ozmux/sdk/host` entry): writes the
//! descriptor JSON, sets the host env, and polls the ready file via run_lifecycle.

use crate::host::{LifecycleEvent, RuntimeRoot, run_lifecycle};
use crossbeam_channel::{Receiver, bounded};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The host's runtime paths + spawn env, with the descriptor JSON already written.
pub struct PreparedHost {
    /// RPC UDS the host binds (Rust connects here in Step 4).
    pub rpc_sock_path: PathBuf,
    /// Descriptor JSON file (`OZMUX_HOST_MANIFEST`) the host reads at startup.
    pub manifest_path: PathBuf,
    /// Ready marker file the host writes after binding; Rust polls its existence.
    pub ready_path: PathBuf,
    /// Env pairs to set on the child (`OZMUX_HOST_*`).
    pub env: Vec<(String, String)>,
}

/// Writes the descriptor JSON into `dir` and assembles the host's paths + env.
///
/// `dir` must be a 0700 runtime directory (e.g. `RuntimeRoot::bin_dir()`).
pub fn prepare_host_runtime(dir: &Path, descriptor_json: &str) -> std::io::Result<PreparedHost> {
    let rpc_sock_path = dir.join("host.rpc.sock");
    let manifest_path = dir.join("host-manifest.json");
    let ready_path = dir.join(".host-ready");
    std::fs::write(&manifest_path, descriptor_json)?;
    let env = vec![
        (
            "OZMUX_HOST_RPC_SOCK".into(),
            rpc_sock_path.to_string_lossy().into_owned(),
        ),
        (
            "OZMUX_HOST_MANIFEST".into(),
            manifest_path.to_string_lossy().into_owned(),
        ),
        (
            "OZMUX_HOST_READY_PATH".into(),
            ready_path.to_string_lossy().into_owned(),
        ),
    ];
    Ok(PreparedHost {
        rpc_sock_path,
        manifest_path,
        ready_path,
        env,
    })
}

/// A running single Node host process.
pub struct HostProcess {
    rpc_sock_path: PathBuf,
    events: Receiver<LifecycleEvent>,
    _runtime: RuntimeRoot,
    child: Arc<std::sync::Mutex<Option<std::process::Child>>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl HostProcess {
    /// Spawns `node <entry>` with the host env, writing `descriptor_json` first
    /// and polling the ready file for up to `ready_timeout`.
    pub fn spawn(
        entry: OsString,
        runtime: RuntimeRoot,
        descriptor_json: &str,
        ready_timeout: Duration,
    ) -> std::io::Result<Self> {
        let prepared = prepare_host_runtime(runtime.bin_dir(), descriptor_json)?;
        let child = Command::new("node")
            .arg(&entry)
            .envs(prepared.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .spawn()?;
        let child = Arc::new(std::sync::Mutex::new(Some(child)));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (tx, rx) = bounded::<LifecycleEvent>(8);
        let ready_path = prepared.ready_path.clone();
        let thread = std::thread::spawn({
            let child = Arc::clone(&child);
            let shutdown = Arc::clone(&shutdown);
            move || {
                run_lifecycle(
                    ready_timeout,
                    move || ready_path.exists(),
                    || {},
                    child,
                    shutdown,
                    tx,
                );
            }
        });
        Ok(Self {
            rpc_sock_path: prepared.rpc_sock_path,
            events: rx,
            _runtime: runtime,
            child,
            shutdown,
            thread: Some(thread),
        })
    }

    /// The RPC socket path the host binds.
    pub fn rpc_sock_path(&self) -> &Path {
        &self.rpc_sock_path
    }

    /// Lifecycle events (`Ready` / `Exited` / `SpawnFailed`) from the supervisor thread.
    pub fn events(&self) -> &Receiver<LifecycleEvent> {
        &self.events
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        // NOTE: signal shutdown before take()/join() so the lifecycle thread
        // observes it and kills the child if it holds the Arc out of the mutex
        // (mirroring CommandExtension::Drop's SeqCst ordering guarantee).
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn prepare_writes_descriptor_and_builds_env() {
        let runtime = tempdir().unwrap();
        let prepared = prepare_host_runtime(runtime.path(), r#"{"plugins":[]}"#).unwrap();

        let written = std::fs::read_to_string(&prepared.manifest_path).unwrap();
        assert_eq!(written, r#"{"plugins":[]}"#);

        let env: std::collections::HashMap<_, _> = prepared.env.iter().cloned().collect();
        assert_eq!(
            env["OZMUX_HOST_RPC_SOCK"],
            prepared.rpc_sock_path.to_string_lossy()
        );
        assert_eq!(
            env["OZMUX_HOST_MANIFEST"],
            prepared.manifest_path.to_string_lossy()
        );
        assert_eq!(
            env["OZMUX_HOST_READY_PATH"],
            prepared.ready_path.to_string_lossy()
        );

        assert!(!prepared.ready_path.exists());
    }
}
