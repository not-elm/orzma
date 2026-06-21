//! Observes `OscWebviewRequest` and mounts/unmounts an inline dynamic webview
//! at the requesting terminal's cursor (the `Mount` / `Unmount`
//! verbs).

use super::mount::{WebviewMountContext, WebviewParams, mount, unmount};
use crate::control_plane::DynamicRegistry;
use bevy::prelude::*;
use ozma_tty_engine::{OscWebviewRequest, OscWebviewVerb};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Shared default-off gate for the OSC-driven webview feature. The same atomic
/// is cloned into every terminal's `SpawnOptions.osc_webview_gate`.
#[derive(Resource, Clone)]
pub(crate) struct OscWebviewGate(pub(crate) Arc<AtomicBool>);

/// Marks a webview as render-only (no pointer or keyboard input
/// forwarded to the embedded page).
#[derive(Component, Debug, Default)]
pub(crate) struct NonInteractive;

/// Wires the OSC-webview mount/unmount observer and the host-supplied gate.
pub(super) struct OscPlugin {
    pub(super) osc_enabled: bool,
}

impl Plugin for OscPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(OscWebviewGate(Arc::new(AtomicBool::new(self.osc_enabled))))
            .add_observer(on_osc_webview_request);
    }
}

pub(crate) fn on_osc_webview_request(
    ev: On<OscWebviewRequest>,
    mut webview: WebviewParams,
    dynamic: Res<DynamicRegistry>,
) {
    let req = ev.event();
    let terminal_surface = req.entity;
    match &req.verb {
        OscWebviewVerb::Mount {
            view_id,
            rows,
            cols,
            instance_id,
        } => {
            mount(
                &mut webview,
                &dynamic,
                WebviewMountContext {
                    terminal_surface,
                    view_id,
                    instance_id: instance_id.as_deref(),
                    rows: *rows,
                    cols: *cols,
                    anchor: req.anchor,
                },
            );
        }
        OscWebviewVerb::Unmount {
            view_id,
            instance_id,
        } => {
            unmount(
                &mut webview,
                terminal_surface,
                view_id.as_deref(),
                instance_id.as_deref(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn osc_plugin_initializes_gate_from_param() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(OscPlugin { osc_enabled: true });
        app.update();
        let gate = app.world().resource::<OscWebviewGate>();
        assert!(
            gate.0.load(Ordering::Relaxed),
            "gate reflects osc_enabled=true"
        );
    }
}
