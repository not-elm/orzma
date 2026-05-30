//! Launches a command (bootstrap-based) extension: spawns `node <main>` with the
//! shim bin dir + command socket + piped stdin, awaits the shim files, and
//! exposes `bin_dir()` for the terminal `PATH` prefix. The shim/command server
//! live in the extension (TS); this only manages the process + readiness.

use crate::host::{HostError, HostResult, LifecycleEvent, RuntimeRoot, run_lifecycle};
use crossbeam_channel::{Receiver, bounded};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// How to launch a command (bootstrap) extension.
pub struct CommandExtensionConfig {
    /// Extension name (also the `EXTENSION_NAME` env + runtime-root key).
    pub name: String,
    /// Extension directory (the child's cwd).
    pub dir: PathBuf,
    /// Entry script, launched as `node <main>` (e.g. `bootstrap.ts`).
    pub main: OsString,
    /// Command names whose shim files signal readiness (e.g. `["@memo"]`).
    pub commands: Vec<String>,
}

/// A running command extension. Owns the runtime root, the piped stdin (the
/// SDK's parent-death channel), and the lifecycle thread; kills the child on drop.
pub struct CommandExtension {
    bin_dir: PathBuf,
    events: Receiver<LifecycleEvent>,
    _runtime: Arc<RuntimeRoot>,
    _stdin: ChildStdin,
    child: Arc<std::sync::Mutex<Option<std::process::Child>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CommandExtension {
    /// Spawns the command extension with the default readiness timeout.
    pub fn spawn(cfg: CommandExtensionConfig) -> HostResult<Self> {
        Self::spawn_with_timeout(cfg, DEFAULT_READY_TIMEOUT)
    }

    /// Spawns with an explicit readiness timeout.
    pub fn spawn_with_timeout(
        cfg: CommandExtensionConfig,
        ready_timeout: Duration,
    ) -> HostResult<Self> {
        let runtime = RuntimeRoot::resolve_in(&std::env::temp_dir(), std::process::id(), &cfg.name)
            .map_err(HostError::Runtime)?;
        let bin_dir = runtime.bin_dir().to_path_buf();
        let command_sock = runtime.socket_path(&cfg.name);
        let handlers_sock = runtime.socket_path(&format!("{}.handlers", cfg.name));

        let mut child = Command::new("node")
            .arg(&cfg.main)
            .current_dir(&cfg.dir)
            .env("OZMUX_BIN_DIR", &bin_dir)
            .env("OZMUX_SOCK_PATH", &command_sock)
            .env("EXTENSION_NAME", &cfg.name)
            .env("OZMUX_HANDLERS_SOCK_PATH", &handlers_sock)
            // NOTE: piping stdin is required — the SDK uses the child's stdin as a
            // parent-death channel; an EOF'd stdin makes bootstrap() self-clean
            // (removing the shim) before readiness can observe it. Holding the
            // write end open keeps it alive; dropping it later is graceful shutdown.
            .stdin(Stdio::piped())
            .spawn()
            .map_err(HostError::Spawn)?;
        let stdin = child.stdin.take().expect("piped stdin");

        let runtime = Arc::new(runtime);
        let child = Arc::new(std::sync::Mutex::new(Some(child)));
        let (tx, rx) = bounded::<LifecycleEvent>(8);

        let thread = std::thread::spawn({
            let child = Arc::clone(&child);
            let bin_dir = bin_dir.clone();
            let commands = cfg.commands.clone();
            move || {
                run_lifecycle(
                    ready_timeout,
                    move || commands.iter().all(|c| bin_dir.join(c).exists()),
                    || {},
                    child,
                    tx,
                );
            }
        });

        Ok(Self {
            bin_dir,
            events: rx,
            _runtime: runtime,
            _stdin: stdin,
            child,
            thread: Some(thread),
        })
    }

    /// The directory holding this extension's command shims (for the PATH prefix).
    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
    }

    /// The lifecycle event stream.
    pub fn events(&self) -> &Receiver<LifecycleEvent> {
        &self.events
    }

    /// Blocks until `Ready`, or returns `NotReady` on `SpawnFailed`/timeout.
    pub fn wait_ready(&self, timeout: Duration) -> HostResult {
        match self.events.recv_timeout(timeout) {
            Ok(LifecycleEvent::Ready) => Ok(()),
            Ok(LifecycleEvent::SpawnFailed { .. }) | Ok(LifecycleEvent::Exited { .. }) => {
                Err(HostError::NotReady)
            }
            Err(_) => Err(HostError::NotReady),
        }
    }
}

impl Drop for CommandExtension {
    fn drop(&mut self) {
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

    fn memo_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extensions/memo")
    }

    fn node_and_memo_available() -> bool {
        let node = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v node")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        node && memo_dir().join("node_modules/@ozmux/sdk").exists()
    }

    #[test]
    fn launches_memo_and_writes_shim() {
        if !node_and_memo_available() {
            eprintln!("skipping: node or memo's @ozmux/sdk link not available");
            return;
        }
        let ext = CommandExtension::spawn(CommandExtensionConfig {
            name: "memo".into(),
            dir: memo_dir(),
            main: "bootstrap.ts".into(),
            commands: vec!["@memo".into()],
        })
        .expect("spawn memo");
        ext.wait_ready(Duration::from_secs(10)).expect("memo ready");
        assert!(
            ext.bin_dir().join("@memo").exists(),
            "@memo shim must be written"
        );
    }
}
