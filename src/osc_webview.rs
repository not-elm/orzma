//! Observes `OscWebviewRequest` and mounts/unmounts an inline dynamic webview
//! at the requesting terminal's cursor (the `MountInline` / `UnmountInline`
//! verbs); the non-inline tab verbs are accepted but no longer acted on.

use crate::control_plane::DynamicRegistry;
use crate::inline_webview::{
    InlineMountContext, InlineWebviewParams, mount_inline, unmount_inline,
};
use bevy::prelude::*;
use ozma_tty_engine::{OscWebviewRequest, OscWebviewVerb};
use ozmux_multiplexer::{MultiplexerCommands, SurfaceOf};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared default-off gate for the OSC-driven webview feature. The same atomic
/// is cloned into every terminal's `SpawnOptions.osc_webview_gate`.
#[derive(Resource, Clone)]
pub(crate) struct OscWebviewGate(pub(crate) Arc<AtomicBool>);

/// Marks an inline webview as render-only (no pointer or keyboard input
/// forwarded to the embedded page).
#[derive(Component, Debug, Default)]
pub(crate) struct NonInteractive;

/// The host-API namespaces a webview is permitted to call (the per-webview
/// capability grant the dormant host-RPC bridge reads). It is the in-ECS trust
/// record, never derived from webview-supplied data. Currently always empty:
/// the per-webview API registration that would populate it is not yet wired.
#[derive(Component, Debug, Clone, Default)]
pub(crate) struct GrantedNamespaces(pub(crate) HashSet<String>);

/// Wires the OSC-webview mount/unmount observer and the config-driven gate.
pub(crate) struct OzmuxOscWebviewPlugin;

impl Plugin for OzmuxOscWebviewPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(OscWebviewGate(Arc::new(AtomicBool::new(false))))
            .add_systems(Startup, init_gate_from_config)
            .add_observer(on_osc_webview_request);
    }
}

fn init_gate_from_config(
    gate: Res<OscWebviewGate>,
    configs: Res<crate::configs::OzmuxConfigsResource>,
) {
    gate.0.store(configs.osc_webview.enabled, Ordering::Relaxed);
}

pub(crate) fn on_osc_webview_request(
    ev: On<OscWebviewRequest>,
    mut inline: InlineWebviewParams,
    mux: MultiplexerCommands,
    dynamic: Res<DynamicRegistry>,
    surface_of: Query<&SurfaceOf>,
) {
    let req = ev.event();
    let terminal_surface = req.entity;
    let Ok(pane) = surface_of.get(terminal_surface).map(|s| s.0) else {
        return;
    };
    match &req.verb {
        OscWebviewVerb::MountInline {
            view_id,
            rows,
            cols,
            instance_id,
        } => {
            let Some(workspace) = mux.workspace_of_pane(pane) else {
                tracing::debug!(%view_id, "osc-webview: mount-inline from a pane with no workspace, dropping");
                return;
            };
            mount_inline(
                &mut inline,
                &dynamic,
                InlineMountContext {
                    terminal_surface,
                    workspace,
                    pane,
                    view_id,
                    instance_id: instance_id.as_deref(),
                    rows: *rows,
                    cols: *cols,
                    anchor: req.anchor,
                },
            );
        }
        OscWebviewVerb::UnmountInline {
            view_id,
            instance_id,
        } => {
            unmount_inline(
                &mut inline,
                terminal_surface,
                view_id.as_deref(),
                instance_id.as_deref(),
            );
        }
        // The non-inline tab-mount verbs are still parsed by the VT layer but no
        // longer act on anything (their tab-surface path was removed). Log the
        // drop so a program still emitting them isn't met with total silence.
        OscWebviewVerb::Mount { .. } | OscWebviewVerb::Unmount { .. } => {
            tracing::debug!(
                verb = ?req.verb,
                "osc-webview: non-inline mount/unmount verb is no longer supported, dropping"
            );
        }
    }
}
