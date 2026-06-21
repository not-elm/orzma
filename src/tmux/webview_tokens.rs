//! Host overlay: binds each tmux pane's `%N` id (matching `$TMUX_PANE`) to its
//! entity in `ozma_webview`'s token registry, so a program's `hello %N` resolves
//! to the pane that owns it. This is the multiplexer-specific half of webview
//! token resolution; the generic per-surface `$OZMA_TOKEN` path lives in the
//! webview crate.

use bevy::prelude::*;
use ozma_webview::ControlPlaneHandle;
use ozmux_tmux::TmuxPane;

/// Registers the tmux `%N` token binder, gated to `AppMode::Ozmux` via
/// `OzmuxActiveSet`.
pub(crate) struct WebviewTokensPlugin;

impl Plugin for WebviewTokensPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, bind_tmux_pane_tokens.in_set(super::OzmuxActiveSet));
    }
}

/// Binds `%<pane-id>` → pane entity for every newly projected tmux pane.
fn bind_tmux_pane_tokens(
    new_panes: Query<(Entity, &TmuxPane), Added<TmuxPane>>,
    handle: Option<Res<ControlPlaneHandle>>,
) {
    let Some(handle) = handle else {
        return;
    };
    for (entity, pane) in new_panes.iter() {
        handle.tokens.insert(format!("%{}", pane.id.0), entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_webview::TokenRegistry;
    use ozmux_tmux::PaneId;
    use std::path::PathBuf;
    use tmux_control_parser::CellDims;

    #[test]
    fn binds_pane_id_token_to_entity() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let tokens = TokenRegistry::default();
        app.insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/x.sock"),
            tokens: tokens.clone(),
        });
        app.add_systems(Update, bind_tmux_pane_tokens);

        let dims = CellDims {
            width: 80,
            height: 24,
            xoff: 0,
            yoff: 0,
        };
        let pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(7),
                dims,
            })
            .id();
        app.update();

        assert_eq!(
            tokens.resolve("%7"),
            Some(pane),
            "%N resolves to its pane entity"
        );
    }
}
