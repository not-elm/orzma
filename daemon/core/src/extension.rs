use {
    crate::{
        http::AppState,
        session::{SessionId, SessionState, pane::PaneId},
    },
    interprocess::local_socket::{
        GenericFilePath, GenericNamespaced, ListenerOptions, Name, NameType, ToFsName, ToNsName,
        tokio::prelude::*,
    },
    serde::Deserialize,
    tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        try_join,
    },
};

pub mod manifest;
pub mod state;

pub fn serve(state: AppState) {
    tokio::spawn(async move {
        if let Err(e) = _serve(state).await {
            tracing::error!("extension host socket server stopped: {e}");
        }
    });
}

pub async fn _serve(state: AppState) -> std::io::Result<()> {
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
                    Ok(_) => println!("recive: {buffer}"),
                }
            }
        });
    }
}

fn resolve_socket_name() -> std::io::Result<Name<'static>> {
    const SOCKET_NAME: &str = "ozmux-extension-host.sock";
    if GenericNamespaced::is_supported() {
        // Windows
        SOCKET_NAME.to_ns_name::<GenericNamespaced>()
    } else {
        // Unix
        let path = std::env::temp_dir().join(SOCKET_NAME);
        if std::fs::exists(&path)? {
            std::fs::remove_file(&path)?;
        }
        path.to_fs_name::<GenericFilePath>()
    }
}

#[derive(Deserialize)]
struct CreateActivity {
    pub pane: PaneId,
    pub view_path: String,
}
