//! Observes the APC webview signals and mounts / unmounts an inline
//! dynamic webview on the requesting terminal.

use super::mount::{WebviewMountContext, WebviewParams, mount, unmount};
use crate::control_plane::OrzmaRegistry;
use bevy::prelude::*;
use bevy_orzma_tty::prelude::{
    TtyWebviewMountRejectedSignal, TtyWebviewMountSignal, TtyWebviewUnmountSignal,
};

/// Marks a webview as render-only (no pointer or keyboard input
/// forwarded to the embedded page).
#[derive(Component, Debug, Default)]
pub struct NonInteractive;

/// Wires the APC-webview mount / unmount observers.
pub(crate) struct ApcPlugin;

impl Plugin for ApcPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_webview_mount)
            .add_observer(on_webview_mount_rejected)
            .add_observer(on_webview_unmount);
    }
}

/// Mounts (or updates the placement of) the webview the VT signal names.
pub(crate) fn on_webview_mount(
    ev: On<TtyWebviewMountSignal>,
    mut webview: WebviewParams,
    dynamic: Res<OrzmaRegistry>,
) {
    let req = ev.event();
    mount(
        &mut webview,
        &dynamic,
        WebviewMountContext {
            terminal_surface: req.terminal,
            instance: req.instance,
            rows: req.size.rows,
            cols: req.size.cols,
        },
    );
}

/// Reports a mount the VT refused. The placement cap rejected it, so
/// there is nothing to spawn — without this line a webview that never
/// appears would leave no trace at all.
pub(crate) fn on_webview_mount_rejected(ev: On<TtyWebviewMountRejectedSignal>) {
    let req = ev.event();
    tracing::debug!(
        instance = %req.instance,
        "apc-webview: mount rejected by the VT, dropping"
    );
}

/// Unmounts the webview(s) the VT signal names.
pub(crate) fn on_webview_unmount(ev: On<TtyWebviewUnmountSignal>, mut webview: WebviewParams) {
    let req = ev.event();
    unmount(&mut webview, req.terminal, req.instance);
}
