#[cfg(not(target_os = "linux"))]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(target_os = "linux")]
use interprocess::local_socket::{GenericNamespaced, NameType, ToNsName};
use interprocess::local_socket::{ListenerOptions, Name, tokio::prelude::*};
use ozmux_session::SessionState;

use crate::error::ExtensionResult;

/// Spawn the extension host UDS listener on a tokio task.
///
/// Currently no IPC requests are defined; the listener accepts and
/// immediately drops connections. The scaffold is preserved so future
/// session-mutating commands can be added without re-introducing the
/// transport layer.
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
        drop(conn);
    }
}

const SOCKET_NAME: &str = "ozmux-extension-host.sock";

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
