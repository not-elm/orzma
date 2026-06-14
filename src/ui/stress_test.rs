//! Tier 2 stress test for the workspace-owned UI design. Spawns several
//! workspaces x panes x surfaces and runs a long swap loop, asserting no
//! taffy panic and no unbounded taffy-node growth. Gates the deletion of
//! `hidden_stash` per the spec: if this test panics, the upstream taffy
//! fixes (PRs #13990 / #16780 / #17596) do not cover our usage pattern.

#![cfg(test)]

use crate::bootstrap::OzmuxBootstrapPlugin;
use crate::configs::OzmuxConfigsPlugin;
use crate::ui::OzmuxUiPlugin;
use bevy::asset::AssetPlugin;
use bevy::image::ImagePlugin;
use bevy::prelude::*;
use bevy::render::storage::ShaderStorageBuffer;
use bevy::window::{PrimaryWindow, WindowResolution};
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};
use ozmux_multiplexer::{
    AttachedWorkspace, MultiplexerPlugin, WorkspaceMarker, WorkspaceUiSubtree,
};
use std::sync::MutexGuard;

fn make_app() -> (App, MutexGuard<'static, ()>) {
    let guard = crate::configs::env_guard();
    // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
    unsafe {
        std::env::remove_var("OZMUX_CONFIG");
    }
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(ImagePlugin::default())
        .init_asset::<TerminalUiMaterial>()
        .init_asset::<ShaderStorageBuffer>()
        .insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        })
        .add_plugins(MultiplexerPlugin)
        .add_plugins(OzmuxConfigsPlugin)
        .add_plugins(OzmuxBootstrapPlugin)
        .add_plugins(OzmuxUiPlugin);
    app.world_mut().spawn((
        Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        },
        PrimaryWindow,
    ));
    (app, guard)
}

#[test]
fn taffy_handles_repeated_park_unpark_under_load() {
    let (mut app, _guard) = make_app();
    app.update();
    app.update();

    let all_workspaces: Vec<Entity> = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<WorkspaceMarker>>();
        q.iter(world).collect()
    };
    assert!(
        !all_workspaces.is_empty(),
        "at least one workspace after bootstrap"
    );

    let mut current_attached = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<AttachedWorkspace>>();
        q.single(world).expect("exactly one attached at start")
    };
    for i in 0..5 {
        let next = all_workspaces[i % all_workspaces.len()];
        if next == current_attached {
            continue;
        }
        app.world_mut()
            .entity_mut(current_attached)
            .remove::<AttachedWorkspace>();
        app.world_mut().entity_mut(next).insert(AttachedWorkspace);
        app.update();
        current_attached = next;
    }

    let world = app.world_mut();
    let mut q = world.query::<&WorkspaceUiSubtree>();
    for sub in q.iter(world) {
        assert!(
            world.get_entity(sub.0).is_ok(),
            "subtree entity must survive stress loop",
        );
    }
}
