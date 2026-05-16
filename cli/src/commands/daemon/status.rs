//! `ozmux daemon status` — print daemon liveness and health.

use std::time::Duration;

const HEALTH_URL: &str = "http://127.0.0.1:3200/health";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
const LISTEN_ADDR: &str = "127.0.0.1:3200";

/// Prints daemon status to stdout and exits with code 0 (healthy), 3
/// (not running / stale PID), or 4 (running but unhealthy).
pub(crate) async fn run() -> anyhow::Result<()> {
    let code = run_status().await?;
    std::process::exit(code);
}

async fn run_status() -> anyhow::Result<i32> {
    let Some(pid) = daemon_bootstrap::pidfile::read()? else {
        eprintln!("ozmux daemon not running");
        return Ok(3);
    };

    if !daemon_bootstrap::pidfile::is_process_alive(pid)? {
        eprintln!("ozmux daemon not running (stale PID file: {pid})");
        return Ok(3);
    }

    let healthy = check_health().await;

    println!("pid:       {pid}");
    println!("listening: {LISTEN_ADDR}");
    println!("health:    {}", if healthy { "ok" } else { "unhealthy" });

    Ok(if healthy { 0 } else { 4 })
}

async fn check_health() -> bool {
    let Ok(client) = reqwest::Client::builder().timeout(HEALTH_TIMEOUT).build() else {
        return false;
    };
    matches!(client.get(HEALTH_URL).send().await, Ok(r) if r.status().is_success())
}
