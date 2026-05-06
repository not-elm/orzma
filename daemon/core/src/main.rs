pub use ozmux_macros::define_string_new_type;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ozmux_core=info,warn")),
        )
        .init();
    let state = ozmux_http_server::AppState::default();
    ozmux_extension_host::serve(state.sessions.clone());
    ozmux_http_server::serve(state).await.unwrap();
}
