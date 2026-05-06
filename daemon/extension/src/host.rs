use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, NameType, ToFsName, ToNsName,
    tokio::prelude::*,
};
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

fn resolve_socket_name() -> std::io::Result<Name<'static>> {
    const SOCKET_NAME: &str = "ozmux-extension-host.sock";
    if GenericNamespaced::is_supported() {
        SOCKET_NAME.to_ns_name::<GenericNamespaced>()
    } else {
        let path = std::env::temp_dir().join(SOCKET_NAME);
        if std::fs::exists(&path)? {
            std::fs::remove_file(&path)?;
        }
        path.to_fs_name::<GenericFilePath>()
    }
}
