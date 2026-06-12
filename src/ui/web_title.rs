//! Surface tab-title sync with the CEF webview page title.

use bevy::prelude::*;
use bevy_cef::prelude::TitleChanged;
use ozmux_multiplexer::SurfaceMarker;

/// The live webview page title for an extension surface, mirrored from
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
/// event targets the webview entity: an extension Surface, which is itself
/// the webview.
fn on_webview_title_changed(
    ev: On<TitleChanged>,
    mut commands: Commands,
    surfaces: Query<(), With<SurfaceMarker>>,
) {
    if surfaces.get(ev.webview).is_ok() {
        // NOTE: try_insert (not insert) — a CEF title queued for a just-closed
        // pane can target an already-despawned Surface entity; skip, don't error.
        commands
            .entity(ev.webview)
            .try_insert(WebTitle(ev.title.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_writes_web_title_for_extension() {
        let mut app = App::new();
        app.add_observer(on_webview_title_changed);
        // Extension: the webview entity IS the Surface.
        let surface = app.world_mut().spawn(SurfaceMarker).id();

        app.world_mut().trigger(TitleChanged {
            webview: surface,
            title: "memo".into(),
        });
        app.world_mut().flush();

        assert_eq!(app.world().get::<WebTitle>(surface).unwrap().0, "memo");
    }
}
