use interprocess::local_socket::tokio::Listener;
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use ozmux_mux::MultiPlexer;
use ozmux_proto::ClientMessage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct OzmuxServer {
    listener: Listener,
    multiplexer: Arc<MultiPlexer>,
}

impl OzmuxServer {
    pub fn new() -> anyhow::Result<Self> {
        let name = "ozmux-daemon.sock".to_ns_name::<GenericNamespaced>()?;
        let listener = ListenerOptions::new().name(name).create_tokio()?;
        let multiplexer = Arc::new(MultiPlexer::default());
        Ok(Self {
            listener,
            multiplexer,
        })
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        loop {
            let conn = self.listener.accept().await?;
            let mux = self.multiplexer.clone();
            tokio::spawn(async move {
                if let Err(error) = handle_client(conn, mux).await {
                    tracing::error!(?error);
                }
            });
        }
    }
}

async fn handle_client(
    stream: interprocess::local_socket::tokio::Stream,
    multiplexer: MultiPlexer,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    loop {
        line.clear();

        let n = reader.read_line(&mut line).await?;

        if n == 0 {
            break;
        }
        let Ok(request) = serde_json::from_str::<ClientMessage>(&line) else {
            continue;
        };
        todo!("ClientMessageをパターンマッチし、適切な処理を行う。");
        match request {
            _ => todo!(),
        }

        let response = format!("echo: {line}");
        write_half.write_all(response.as_bytes()).await?;
        write_half.flush().await?;
    }

    Ok(())
}
