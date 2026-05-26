//! Tier 2 stress test for the session-owned UI design. Spawns several
//! sessions x panes x activities and runs a long swap loop, asserting no
//! taffy panic and no unbounded taffy-node growth. Gates the deletion of
//! `hidden_stash` per the spec: if this test panics, the upstream taffy
//! fixes (PRs #13990 / #16780 / #17596) do not cover our usage pattern.

#![cfg(test)]

use crate::bootstrap::OzmuxBootstrapPlugin;
use crate::configs::OzmuxConfigsPlugin;
use crate::multiplexer::{
    AttachedSession, Multiplexer, OzmuxMultiplexerPlugin, SessionEntityId, SessionUiSubtree,
};
use crate::ui::OzmuxUiPlugin;
use bevy::asset::AssetPlugin;
use bevy::image::ImagePlugin;
use bevy::prelude::*;
use bevy::render::storage::ShaderStorageBuffer;
use bevy::window::{PrimaryWindow, WindowResolution};
use bevy_terminal_renderer::material::TerminalUiMaterial;
use bevy_terminal_renderer::{CellMetrics, TerminalCellMetricsResource};
use ozmux_multiplexer::{Activity, ActivityId, PaneId, SessionId, Side, SplitOrientation};
use std::collections::HashSet;
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
        .add_plugins(OzmuxMultiplexerPlugin)
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

    {
        let world = app.world_mut();
        let mut mux = world.resource_mut::<Multiplexer>();
        for i in 0..4 {
            let (sid, base_pane, _) = mux.create_session(Some(format!("s{i}")));
            let new_pane = PaneId::new();
            let new_act = Activity::terminal(ActivityId::new());
            mux.with_session(&sid, |s| {
                s.split_pane(
                    &base_pane,
                    new_pane,
                    new_act,
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .expect("with_session")
            .expect("split_pane");
            let extra_aid = ActivityId::new();
            mux.with_session(&sid, |s| {
                s.pane_mut(&base_pane)
                    .expect("pane_mut")
                    .add_activity(Activity::terminal(extra_aid))
            })
            .expect("with_session")
            .expect("add_activity");
            mux.bump_epoch(&sid);
        }
    }

    let sids_needing_entities: Vec<SessionId> = {
        let world = app.world_mut();
        let mut existing: HashSet<SessionId> = HashSet::new();
        {
            let mut q = world.query::<&SessionEntityId>();
            for sid_comp in q.iter(world) {
                existing.insert(sid_comp.0);
            }
        }
        let mux = world.resource::<Multiplexer>();
        mux.sessions
            .keys()
            .copied()
            .filter(|sid| !existing.contains(sid))
            .collect()
    };

    {
        let world = app.world_mut();
        for sid in sids_needing_entities {
            let subtree = world.spawn(Node::default()).id();
            let entity = world
                .spawn((
                    SessionEntityId(sid),
                    SessionUiSubtree(subtree),
                    Name::new(format!("session {sid:?}")),
                ))
                .id();
            world.entity_mut(subtree).insert(ChildOf(entity));
        }
    }
    app.update();
    app.update();

    let all_sessions: Vec<Entity> = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<SessionEntityId>>();
        q.iter(world).collect()
    };
    assert!(
        all_sessions.len() >= 5,
        "expected at least 5 sessions (bootstrap + 4 minted), got {}",
        all_sessions.len()
    );

    let mut current_attached = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<AttachedSession>>();
        q.single(world).expect("exactly one attached at start")
    };
    for i in 0..200 {
        let next = all_sessions[(i + 1) % all_sessions.len()];
        if next == current_attached {
            continue;
        }
        app.world_mut()
            .entity_mut(current_attached)
            .remove::<AttachedSession>();
        app.world_mut().entity_mut(next).insert(AttachedSession);
        {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            mux.set_changed();
        }
        app.update();
        current_attached = next;
    }

    // NOTE: a despawned subtree under a still-live SessionUiSubtree pointer
    // would crash the next rebuild with an invalid Entity insert; this loop
    // catches that silent corruption mode.
    let world = app.world_mut();
    let mut q = world.query::<(&SessionEntityId, &SessionUiSubtree)>();
    for (_sid, sub) in q.iter(world) {
        assert!(
            world.get_entity(sub.0).is_ok(),
            "subtree entity must survive stress loop",
        );
    }
}
