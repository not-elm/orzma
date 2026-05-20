//! `OZMUX_METRICS=1`-gated Prometheus exposition. Installs a process-global
//! recorder on first use and serves rendered metrics at `/metrics`.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;
use std::time::Duration;

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Installs the global Prometheus recorder if `OZMUX_METRICS=1` and not
/// already installed. Returns `Some(&handle)` when the recorder is active.
pub fn maybe_install() -> Option<&'static PrometheusHandle> {
    if !matches!(std::env::var("OZMUX_METRICS").as_deref(), Ok("1")) {
        return None;
    }
    let _ = HANDLE.get_or_init(|| {
        let handle = PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Full("ozmux_terminal_emit_duration_seconds".to_string()),
                &[0.0001, 0.0005, 0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1],
            )
            .expect("set emit_duration buckets")
            .set_buckets_for_metric(
                Matcher::Full("ozmux_terminal_coalesce_wait_seconds".to_string()),
                &[0.0005, 0.001, 0.003, 0.006, 0.012, 0.025, 0.05, 0.1],
            )
            .expect("set coalesce_wait buckets")
            .install_recorder()
            .expect("install prometheus recorder");
        let upkeep_handle = handle.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                upkeep_handle.run_upkeep();
            }
        });
        handle
    });
    HANDLE.get()
}

/// Axum handler for `GET /metrics`. Returns 404 when the recorder is not
/// installed (i.e. `OZMUX_METRICS` was not set at start-up).
pub async fn metrics_handler() -> impl IntoResponse {
    match HANDLE.get() {
        Some(h) => (StatusCode::OK, h.render()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
