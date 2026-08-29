//! Observes `TtyApcWebviewSignal` and mounts/unmounts an inline dynamic
//! webview on the requesting terminal (the `Mount` / `Unmount` verbs).

use super::mount::{WebviewMountContext, WebviewParams, mount, unmount};
use crate::control_plane::OrzmaRegistry;
use bevy::prelude::*;
use bevy_orzma_tty::prelude::TtyApcWebviewSignal;
use orzma_vt::prelude::WebviewApcVerb;

/// Marks a webview as render-only (no pointer or keyboard input
/// forwarded to the embedded page).
#[derive(Component, Debug, Default)]
pub struct NonInteractive;

/// Wires the APC-webview mount/unmount observer.
pub(crate) struct OscPlugin;

impl Plugin for OscPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_apc_webview_signal);
    }
}

pub(crate) fn on_apc_webview_signal(
    ev: On<TtyApcWebviewSignal>,
    mut webview: WebviewParams,
    dynamic: Res<OrzmaRegistry>,
) {
    let req = ev.event();
    let terminal_surface = req.terminal;
    match &req.verb {
        WebviewApcVerb::Mount {
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
                    placement: req.placement,
                },
            );
        }
        WebviewApcVerb::Unmount {
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
