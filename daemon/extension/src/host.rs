#[cfg(not(target_os = "linux"))]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(target_os = "linux")]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use interprocess::local_socket::{ListenerOptions, Name, tokio::Stream, tokio::prelude::*};
use ozmux_session::SessionState;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::error::ExtensionResult;

/// Spawn the extension host UDS listener on a tokio task.
///
/// Each accepted connection is handled on its own task: NDJSON frames
/// are read line-by-line and dispatched. Today only `register_commands`
/// is recognized — it is logged. Storage of the registration is left to
/// a follow-up.
pub fn serve(sessions: SessionState) {
    tokio::spawn(async move {
        if let Err(e) = run(sessions).await {
            tracing::error!("extension host socket server stopped: {e}");
        }
    });
}

async fn run(_sessions: SessionState) -> ExtensionResult {
    let listener = ListenerOptions::new()
        .name(resolve_socket_name()?)
        .create_tokio()?;

    loop {
        let conn = listener.accept().await?;
        tokio::spawn(handle_connection(conn));
    }
}

async fn handle_connection(conn: Stream) {
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => return,
            Ok(_) => dispatch(line.trim_end()),
            Err(e) => {
                tracing::warn!("extension connection read error: {e}");
                return;
            }
        }
    }
}

fn dispatch(line: &str) {
    if line.is_empty() {
        return;
    }
    match serde_json::from_str::<FromExtension>(line) {
        Ok(FromExtension::RegisterCommands {
            extension_name,
            commands,
        }) => {
            tracing::info!(
                extension = %extension_name,
                ?commands,
                "extension registered commands"
            );
        }
        Err(e) => {
            tracing::warn!("unknown or malformed extension frame: {e}; line={line}");
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FromExtension {
    RegisterCommands {
        #[serde(rename = "extensionName")]
        extension_name: String,
        commands: Vec<String>,
    },
}

const SOCKET_NAME: &str = "ozmux-extension-host.sock";

/// Path string that Node's `net.connect` (and our own listener) use to
/// reach the extension host socket. Shared between the host bind site
/// and the env var passed to spawned extensions so both ends agree.
#[cfg(target_os = "linux")]
pub(crate) fn resolve_socket_path() -> String {
    // Linux abstract namespace: leading NUL byte, no filesystem entry.
    format!("\0{SOCKET_NAME}")
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn resolve_socket_path() -> String {
    std::env::temp_dir()
        .join(SOCKET_NAME)
        .to_string_lossy()
        .into_owned()
}

#[cfg(target_os = "linux")]
fn resolve_socket_name() -> std::io::Result<Name<'static>> {
    // Linux abstract namespace sockets are ephemeral (vanish when the
    // last fd closes), so no filesystem cleanup is needed.
    SOCKET_NAME.to_ns_name::<GenericNamespaced>()
}

#[cfg(not(target_os = "linux"))]
fn resolve_socket_name() -> std::io::Result<Name<'static>> {
    // On non-Linux Unix (notably macOS), `GenericNamespaced::is_supported()`
    // returns true but interprocess maps the namespaced name to a real
    // file at `/tmp/<name>` that survives process exit and breaks the
    // next bind with EADDRINUSE. Use an explicit filesystem path so we
    // know exactly where the socket lives and can clean it up before
    // binding.
    let path = std::env::temp_dir().join(SOCKET_NAME);
    let _ = std::fs::remove_file(&path);
    path.to_fs_name::<GenericFilePath>()
}
