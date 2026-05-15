use crate::AppState;
use axum::{
    Router,
    routing::{get, post},
};
use ozmux_multiplexer::{Activity, ActivityId, ActivityKind};
use serde::Deserialize;
use std::path::PathBuf;

pub mod activate;
pub mod add_to_pane;
pub mod handlers_ws;
pub mod iframe_serve;
pub mod terminal_ws;
mod vt_ws;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(add_to_pane::add_to_pane))
        .route("/{activity_id}/activate", post(activate::activate))
        .route("/{activity_id}/terminal/ws", get(terminal_ws::terminal_ws))
        .route("/{activity_id}/handlers/ws", get(handlers_ws::handlers_ws))
        .route(
            "/{activity_id}/iframe/{*path}",
            get(iframe_serve::iframe_serve),
        )
}

#[derive(Deserialize)]
pub struct ActivityInput {
    activity_id: ActivityId,
    #[serde(default)]
    name: Option<String>,
    kind: ActivityKindInput,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ActivityKindInput {
    Terminal,
    Extension {
        html_root: PathBuf,
        /// Owning extension's name. The daemon uses this to populate the
        /// `ExtensionRegistry` so subsequent iframe / handlers-WS requests
        /// can route to the right extension UDS. Required for the Extension
        /// variant; the SDK fills it from the bootstrap-time `EXTENSION_NAME`
        /// env var.
        extension_name: String,
    },
}

/// Result of parsing the wire `ActivityInput`: the domain `Activity` plus the
/// owning extension's name when the activity is Extension-kind. The name is
/// not stored on `Activity` itself (the multiplexer model has no notion of an
/// "owner"); the handler uses it to populate `ExtensionRegistry`.
pub(super) struct ParsedActivity {
    pub activity: Activity,
    pub extension_name: Option<String>,
}

impl ActivityInput {
    /// Convert the wire payload into a domain `Activity`, also surfacing the
    /// owning extension's name for Extension-kind activities.
    pub(super) fn into_parsed(self) -> ParsedActivity {
        let (kind, extension_name) = match self.kind {
            ActivityKindInput::Terminal => (ActivityKind::Terminal, None),
            ActivityKindInput::Extension {
                html_root,
                extension_name,
            } => (ActivityKind::Extension { html_root }, Some(extension_name)),
        };
        let activity = Activity {
            id: self.activity_id,
            name: self.name.unwrap_or_else(|| "Activity".into()),
            kind,
        };
        ParsedActivity {
            activity,
            extension_name,
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::AppState;
    use crate::test_helpers;
    use ozmux_multiplexer::{Activity, ActivityId, PaneId, WindowId};
    use ozmux_terminal::SpawnOptions;
    use tokio::net::TcpListener;

    /// Boot a full daemon router with the bootstrap session and a PTY spawned
    /// for the initial activity. Returns the listen address plus the IDs of the
    /// bootstrap (window, pane, activity).
    pub(crate) async fn boot_server_full()
    -> (std::net::SocketAddr, AppState, WindowId, PaneId, ActivityId) {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, activity_id) = test_helpers::bootstrap_default(&state).await;
        state
            .terminal
            .spawn(
                pid.clone(),
                activity_id.clone(),
                SpawnOptions {
                    cols: 80,
                    rows: 24,
                    shell: "/bin/sh".to_string(),
                    cwd: None,
                    window_id: None,
                    session_id: None,
                },
            )
            .await
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::test_helpers::daemon_router_for_test(state.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, state, wid, pid, activity_id)
    }

    /// Build a router with the bootstrap session plus an extension Activity
    /// hosted inside the initial Pane so the hierarchical iframe / WS routes
    /// can validate (wid, pid, aid) and serve files from `html_root`.
    pub(crate) async fn setup_hierarchical_extension(
        html_body: &[u8],
    ) -> (
        axum::Router,
        AppState,
        WindowId,
        PaneId,
        ActivityId,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("index.html"), html_body).unwrap();
        std::fs::write(tmp.path().join("style.css"), b"body { color: red; }").unwrap();
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _initial_aid) = test_helpers::bootstrap_default(&state).await;
        state.extensions.register("memo", tmp.path());
        let activity = Activity::extension(ActivityId::new(), "ext", tmp.path().to_path_buf());
        let aid = activity.id.clone();
        state
            .multiplexer
            .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.add_activity(activity))
            .await
            .unwrap();
        state.extensions.record_activity_owner(&aid, "memo");
        let (router, _) = test_helpers::router_with(state.clone());
        (router, state, wid, pid, aid, tmp)
    }
}
