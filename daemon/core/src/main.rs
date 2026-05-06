use crate::http::AppState;
pub use ozmux_macros::define_string_new_type;

mod error;
mod http;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ozmux_core=info,warn")),
        )
        .init();
    let state = AppState::default();
    ozmux_extension_host::serve(state.sessions.clone());
    http::serve(state).await.unwrap();
}
