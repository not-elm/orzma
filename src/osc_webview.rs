//! Observes `OscWebviewRequest` and mounts/unmounts a registered webview as a
//! tab in the requesting terminal's pane, reusing the extension-surface path.

use bevy::prelude::*;
use bevy_terminal::{OscWebviewRequest, OscWebviewVerb};
use ozmux_extension_host::ViewRegistry;
use ozmux_multiplexer::{
    ExtensionSurfaceId, MultiplexerCommands, OwningExtension, SurfaceKind, SurfaceOf,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Shared default-off gate for the OSC-driven webview feature. The same atomic
/// is cloned into every terminal's `SpawnOptions.osc_webview_gate`.
#[derive(Resource, Clone)]
pub(crate) struct OscWebviewGate(pub(crate) Arc<AtomicBool>);

/// Marks a surface mounted by the OSC path, recording its view id and the
/// origin terminal surface so unmount is deterministic.
#[derive(Component, Debug, Clone)]
pub(crate) struct OscMounted {
    pub(crate) view_id: String,
    pub(crate) terminal_surface: Entity,
}

/// Marks an OSC-mounted webview surface as render-only (no pointer or keyboard
/// input forwarded to the embedded page).
#[derive(Component, Debug, Default)]
pub(crate) struct NonInteractive;

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

fn on_osc_webview_request(
    ev: On<OscWebviewRequest>,
    mut mux: MultiplexerCommands,
    registry: Res<ViewRegistry>,
    surface_of: Query<&SurfaceOf>,
    mounted: Query<(Entity, &OscMounted, &SurfaceOf)>,
) {
    let req = ev.event();
    let terminal_surface = req.entity;
    let Ok(pane) = surface_of.get(terminal_surface).map(|s| s.0) else {
        return;
    };
    match &req.verb {
        OscWebviewVerb::Mount { view_id } => {
            if let Some((existing, _, _)) = mounted
                .iter()
                .find(|(_, m, so)| m.view_id == *view_id && so.0 == pane)
            {
                let _ = mux.set_active_surface(pane, existing);
                return;
            }
            let Some(view) = registry.get(view_id) else {
                tracing::debug!(%view_id, "osc-webview: unregistered view, ignoring");
                return;
            };
            let interactive = view.interactive;
            let entry = PathBuf::from(&view.entry);
            let owning = view.owning_ext.clone();
            let view_id = view_id.clone();
            let surface = mux.add_surface(pane, SurfaceKind::Extension { entry });
            mux.insert_on(surface, ExtensionSurfaceId(format!("osc:{view_id}")));
            mux.insert_on(surface, OwningExtension(owning));
            mux.insert_on(
                surface,
                OscMounted {
                    view_id,
                    terminal_surface,
                },
            );
            if !interactive {
                mux.insert_on(surface, NonInteractive);
                mux.insert_on(surface, Pickable::IGNORE);
            }
            let _ = mux.set_active_surface(pane, surface);
        }
        OscWebviewVerb::Unmount { view_id } => {
            let target = mounted.iter().find(|(_, m, so)| {
                so.0 == pane && view_id.as_ref().is_none_or(|v| m.view_id == *v)
            });
            if let Some((surface, m, _)) = target {
                let origin = m.terminal_surface;
                let _ = mux.set_active_surface(pane, origin);
                let _ = mux.close_surface(pane, surface);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_extension_host::RegisteredView;
    use ozmux_multiplexer::{ActiveSurface, MultiplexerPlugin, Surfaces};

    fn make_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .init_resource::<ViewRegistry>()
            .add_observer(on_osc_webview_request);
        app
    }

    fn register_view(app: &mut App, view_id: &str, interactive: bool) {
        app.world_mut().resource_mut::<ViewRegistry>().register(
            view_id.into(),
            RegisteredView {
                entry: "ui/dash.html".into(),
                owning_ext: "memo".into(),
                interactive,
            },
        );
    }

    fn surface_count(app: &App, pane: Entity) -> usize {
        app.world()
            .get::<Surfaces>(pane)
            .map(|s| s.iter().count())
            .unwrap_or(0)
    }

    fn active_surface(app: &App, pane: Entity) -> Option<Entity> {
        app.world().get::<ActiveSurface>(pane).map(|a| a.0)
    }

    #[test]
    fn mount_adds_extension_surface_and_switches_active() {
        let mut app = make_test_app();
        register_view(&mut app, "dash", true);

        let (pane, terminal_surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();

        // Trigger mount from the terminal surface.
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal_surface,
            verb: OscWebviewVerb::Mount {
                view_id: "dash".into(),
            },
        });
        app.world_mut().flush();

        assert_eq!(
            surface_count(&app, pane),
            2,
            "mount must add a second surface to the pane"
        );

        let active = active_surface(&app, pane).expect("pane must have an active surface");
        assert_ne!(
            active, terminal_surface,
            "active surface must switch to the mounted webview"
        );
        assert!(
            app.world().get::<OscMounted>(active).is_some(),
            "the new active surface must carry OscMounted"
        );
        assert_eq!(
            app.world()
                .get::<OscMounted>(active)
                .map(|m| m.view_id.as_str()),
            Some("dash"),
        );
    }

    #[test]
    fn unmount_restores_terminal_surface_and_despawns_webview() {
        let mut app = make_test_app();
        register_view(&mut app, "dash", true);

        let (pane, terminal_surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();

        // Mount first.
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal_surface,
            verb: OscWebviewVerb::Mount {
                view_id: "dash".into(),
            },
        });
        app.world_mut().flush();

        let webview_surface = active_surface(&app, pane).expect("webview must be active");

        // Now unmount with no specific view_id.
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal_surface,
            verb: OscWebviewVerb::Unmount { view_id: None },
        });
        app.world_mut().flush();
        // NOTE: despawn is deferred; a flush + update applies it.
        app.update();

        assert_eq!(
            active_surface(&app, pane),
            Some(terminal_surface),
            "unmount must restore the terminal surface as active"
        );
        assert!(
            app.world().get_entity(webview_surface).is_err(),
            "unmount must despawn the webview surface"
        );
        assert_eq!(
            surface_count(&app, pane),
            1,
            "pane must return to a single surface after unmount"
        );
    }

    #[test]
    fn mount_non_interactive_stamps_pickable_ignore() {
        let mut app = make_test_app();
        register_view(&mut app, "hud", false);

        let (_pane, terminal_surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal_surface,
            verb: OscWebviewVerb::Mount {
                view_id: "hud".into(),
            },
        });
        app.world_mut().flush();

        let pane = _pane;
        let active = active_surface(&app, pane).expect("active surface");
        assert!(
            app.world().get::<NonInteractive>(active).is_some(),
            "non-interactive view must carry NonInteractive"
        );
        assert!(
            app.world().get::<Pickable>(active).is_some(),
            "non-interactive view must carry Pickable::IGNORE"
        );
    }

    #[test]
    fn mount_unregistered_view_is_ignored() {
        let mut app = make_test_app();

        let (pane, terminal_surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal_surface,
            verb: OscWebviewVerb::Mount {
                view_id: "ghost".into(),
            },
        });
        app.world_mut().flush();

        assert_eq!(
            surface_count(&app, pane),
            1,
            "unregistered view must not add a surface"
        );
        assert_eq!(
            active_surface(&app, pane),
            Some(terminal_surface),
            "active surface must stay unchanged for unregistered view"
        );
    }

    #[test]
    fn duplicate_mount_reactivates_existing_surface() {
        let mut app = make_test_app();
        register_view(&mut app, "dash", true);

        let (pane, terminal_surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();

        // First mount.
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal_surface,
            verb: OscWebviewVerb::Mount {
                view_id: "dash".into(),
            },
        });
        app.world_mut().flush();

        assert_eq!(surface_count(&app, pane), 2);
        let first_webview = active_surface(&app, pane).unwrap();

        // Second mount of the same view_id — must reactivate, not spawn a new surface.
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal_surface,
            verb: OscWebviewVerb::Mount {
                view_id: "dash".into(),
            },
        });
        app.world_mut().flush();

        assert_eq!(
            surface_count(&app, pane),
            2,
            "duplicate mount must not spawn a second webview surface"
        );
        assert_eq!(
            active_surface(&app, pane),
            Some(first_webview),
            "duplicate mount must reactivate the existing webview surface"
        );
    }
}
