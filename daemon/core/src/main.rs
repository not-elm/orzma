use crate::http::AppState;
pub use ozmux_macros::define_string_new_type;

mod error;
mod extension;
mod http;
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
    let state = AppState::default();
    extension::serve(state.clone());
    http::serve(state).await.unwrap();
}
