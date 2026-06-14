//! Spawns the single Node host process at Startup and polls its lifecycle,
//! driving the `HostRpc` client. The host boots with an empty API set (host RPC
//! plumbing kept dormant); per-webview API registration is not yet wired.

use crate::extension_render::HostRpc;
use bevy::prelude::*;
use ozmux_extension_host::host::{LifecycleEvent, RuntimeRoot};
use ozmux_extension_host::{HostProcess, HostRpcClient};
use std::time::Duration;

const READY_TIMEOUT: Duration = Duration::from_secs(10);
const EMPTY_DESCRIPTOR: &str = r#"{"extensions":[]}"#;

#[derive(Resource)]
struct HostRuntime {
    host: HostProcess,
}

/// Spawns the single host process at Startup and polls its lifecycle every frame.
pub(crate) struct ExtensionManagerPlugin;

impl ExtensionManagerPlugin {
    fn spawn_single_host(&self, app: &mut App) {
        match RuntimeRoot::resolve_in(&std::env::temp_dir(), std::process::id(), "host")
            .map_err(|e| e.to_string())
            .and_then(|rt| {
                HostProcess::spawn(rt, EMPTY_DESCRIPTOR, READY_TIMEOUT).map_err(|e| e.to_string())
            }) {
            Ok(host) => {
                app.insert_resource(HostRuntime { host });
            }
            Err(e) => tracing::error!(error = %e, "failed to spawn single host process"),
        }
    }
}

impl Plugin for ExtensionManagerPlugin {
    fn build(&self, app: &mut App) {
        self.spawn_single_host(app);
        app.add_systems(Update, poll_host_lifecycle);
    }
}

fn poll_host_lifecycle(mut host_rpc: Option<ResMut<HostRpc>>, host: Option<Res<HostRuntime>>) {
    let Some(host) = host else {
        return;
    };
    while let Ok(event) = host.host.events().try_recv() {
        match event {
            LifecycleEvent::Ready => match HostRpcClient::connect(host.host.rpc_sock_path()) {
                Ok(client) => {
                    tracing::info!("single host process ready; RPC connected");
                    if let Some(hr) = host_rpc.as_mut() {
                        hr.set_client(client);
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "single host ready but RPC connect failed");
                    if let Some(hr) = host_rpc.as_mut() {
                        hr.clear_client();
                    }
                }
            },
            LifecycleEvent::SpawnFailed { error } => {
                tracing::error!(%error, "single host failed to become ready");
                if let Some(hr) = host_rpc.as_mut() {
                    hr.clear_client();
                }
            }
            LifecycleEvent::Exited { status } => {
                tracing::warn!(?status, "single host process exited");
                if let Some(hr) = host_rpc.as_mut() {
                    hr.clear_client();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::extension_render::HostRpc;

    #[test]
    fn clearing_the_host_client_drops_stale_in_flight_correlation() {
        let mut hr = HostRpc::default();
        hr.note_in_flight_for_test("0", bevy::prelude::Entity::PLACEHOLDER, "h0");
        assert_eq!(hr.count_in_flight_for_test(), 1);
        hr.clear_client();
        assert_eq!(
            hr.count_in_flight_for_test(),
            0,
            "clear_client wipes stale correlation"
        );
    }
}
