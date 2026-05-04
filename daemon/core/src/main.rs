mod error;
mod http;
mod macros;
mod pty;
mod session;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ozmux_core=info,warn")),
        )
        .init();
    http::launch_server().await.unwrap();
}
