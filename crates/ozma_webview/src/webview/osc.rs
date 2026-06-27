//! Observes `OscWebviewRequest` and mounts/unmounts an inline dynamic webview
//! at the requesting terminal's cursor (the `Mount` / `Unmount`
//! verbs).

use super::mount::{WebviewMountContext, WebviewParams, mount, unmount};
use crate::control_plane::OzmaRegistry;
use bevy::prelude::*;
use ozma_tty_engine::{OscWebviewRequest, OscWebviewVerb};

/// Marks a webview as render-only (no pointer or keyboard input
/// forwarded to the embedded page).
#[derive(Component, Debug, Default)]
pub struct NonInteractive;

/// Wires the OSC-webview mount/unmount observer.
pub(crate) struct OscPlugin;

impl Plugin for OscPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_osc_webview_request);
    }
}

pub(crate) fn on_osc_webview_request(
    ev: On<OscWebviewRequest>,
    mut webview: WebviewParams,
    dynamic: Res<OzmaRegistry>,
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
