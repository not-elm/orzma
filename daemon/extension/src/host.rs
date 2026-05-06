use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, NameType, ToFsName, ToNsName,
    tokio::prelude::*,
};
use ozmux_session::SessionState;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Spawn the extension host listener on a tokio task.
///
/// Currently the loop only logs received lines. Future commands that
/// mutate session state will use the supplied `SessionState`.
pub fn serve(sessions: SessionState) {
    tokio::spawn(async move {
        if let Err(e) = run(sessions).await {
            tracing::error!("extension host socket server stopped: {e}");
        }
    });
}

async fn run(_sessions: SessionState) -> std::io::Result<()> {
    let listener = ListenerOptions::new()
        .name(resolve_socket_name()?)
        .create_tokio()?;

    loop {
        let conn = listener.accept().await?;
        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut recver = BufReader::new(&conn);
            loop {
                buffer.clear();
                match recver.read_line(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => tracing::debug!(line = %buffer.trim_end(), "received line"),
                }
            }
        });
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
