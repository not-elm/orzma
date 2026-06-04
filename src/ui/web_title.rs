//! Surface tab-title sync with the CEF webview page title.

use crate::ui::{HostSurfaceEntity, PageWebviewOf};
use bevy::prelude::*;
use bevy_cef::prelude::TitleChanged;

/// The live webview page title for a browser/extension surface, mirrored from
/// bevy_cef's `WebviewTitle` onto the multiplexer Surface entity so `tab_label`
/// and the chrome-dirty hook can read it like `Cwd`.
#[derive(Component, Debug, Clone, Default)]
pub(crate) struct WebTitle(pub(crate) String);

/// Plugin wiring the CEF page-title → Surface `WebTitle` observer.
pub(crate) struct WebTitlePlugin;

impl Plugin for WebTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_webview_title_changed);
    }
}

/// Mirrors a CEF page-title change onto its owning Surface's `WebTitle`. The
/// event targets the webview entity: a browser page child (resolved to its host
/// via `PageWebviewOf`) or an extension host (which is itself the webview).
fn on_webview_title_changed(
    ev: On<TitleChanged>,
    mut commands: Commands,
    page_links: Query<&PageWebviewOf>,
    hosts: Query<&HostSurfaceEntity>,
) {
    let host = page_links.get(ev.webview).map_or(ev.webview, |p| p.0);
    if let Ok(host_surface) = hosts.get(host) {
        // NOTE: try_insert (not insert) — a CEF title queued for a just-closed
        // pane can target an already-despawned Surface entity; skip, don't error.
        commands
            .entity(host_surface.0)
            .try_insert(WebTitle(ev.title.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_writes_web_title_for_browser() {
        let mut app = App::new();
        app.add_observer(on_webview_title_changed);
        let surface = app.world_mut().spawn_empty().id();
        let host = app.world_mut().spawn(HostSurfaceEntity(surface)).id();
        let page = app.world_mut().spawn(PageWebviewOf(host)).id();

        app.world_mut().trigger(TitleChanged {
            webview: page,
            title: "GitHub".into(),
        });
        app.world_mut().flush();

        assert_eq!(app.world().get::<WebTitle>(surface).unwrap().0, "GitHub");
    }

    #[test]
    fn observer_writes_web_title_for_extension() {
        let mut app = App::new();
        app.add_observer(on_webview_title_changed);
        let surface = app.world_mut().spawn_empty().id();
        // Extension: the webview entity IS the host (no PageWebviewOf).
        let host = app.world_mut().spawn(HostSurfaceEntity(surface)).id();

        app.world_mut().trigger(TitleChanged {
            webview: host,
            title: "memo".into(),
        });
        app.world_mut().flush();

        assert_eq!(app.world().get::<WebTitle>(surface).unwrap().0, "memo");
    }
}
