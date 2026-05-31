//! Bevy glue: owns the launched `CommandExtension`, drains its control
//! requests each frame, resolves `OZMUX_PANE_ID` entity bits, and applies the
//! split via `MultiplexerCommands`. Depends on `bevy` + `ozmux_multiplexer`.

use crate::command::{CommandExtension, CommandExtensionConfig, Responder};
use crate::control::{
    ActivityKindSpec, ControlError, ControlOp, ControlOrientation, ControlRequest, ControlResponse,
    ControlSide, SplitReply,
};
use crate::path_prefix::extension_path_prefix;
use bevy::prelude::*;
use ozmux_multiplexer::{
    ActivityKind, ExtensionActivityAid, MultiplexerCommands, Side, SplitOrientation,
};
use std::path::PathBuf;

/// The launched command extension, owned by the app as a Resource.
#[derive(Resource)]
pub struct ControlExtension(pub CommandExtension);

/// System-set label for the control-bridge drain, so a consumer (`ozmux-gui`)
/// can order its UI rebuild after it.
#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum ExtensionControlSet {
    /// `drain_control_requests` — applies pending control requests (e.g. an
    /// `@memo` split) via `MultiplexerCommands`.
    ///
    /// The split mutates `LayoutCells` immediately but spawns the new pane and
    /// inserts its `ActiveActivity` / `ChildOf` through the deferred `Commands`
    /// queue. A UI rebuild that reacts to `Changed<LayoutCells>` MUST run after
    /// this set so the inserted `ApplyDeferred` sync point flushes those
    /// commands first — otherwise the rebuild sees a pane with no activity yet
    /// (no tab, no extension host, no webview).
    Drain,
}

/// Launches the configured command extension at Startup and drains its
/// control requests into the multiplexer each frame.
pub struct ExtensionControlPlugin {
    config: CommandExtensionConfig,
}

impl ExtensionControlPlugin {
    /// Builds the plugin with the extension to launch (e.g. memo).
    pub fn new(config: CommandExtensionConfig) -> Self {
        Self { config }
    }
}

impl Plugin for ExtensionControlPlugin {
    fn build(&self, app: &mut App) {
        match CommandExtension::spawn(self.config.clone()) {
            Ok(ext) => {
                app.insert_resource(ControlExtension(ext));
            }
            Err(e) => eprintln!("ozmux: failed to launch command extension: {e}"),
        }
        app.add_systems(
            Update,
            drain_control_requests
                .in_set(ExtensionControlSet::Drain)
                .run_if(resource_exists::<ControlExtension>),
        );
    }
}

/// Builds the env a terminal must carry so its `@<cmd>` shims work and their
/// control requests reach this host: PATH (extension bin dir first),
/// `OZMUX_PANE_ID`/`OZMUX_SESSION_ID` (entity bits), `OZMUX_CONTROL_SOCK_PATH`.
pub fn terminal_env(
    ext: &CommandExtension,
    pane: Entity,
    session: Entity,
) -> Vec<(String, String)> {
    let current = std::env::var("PATH").unwrap_or_default();
    let bin = ext.bin_dir().to_path_buf();
    vec![
        ("PATH".into(), extension_path_prefix(&[bin], &current)),
        ("OZMUX_PANE_ID".into(), pane.to_bits().to_string()),
        ("OZMUX_SESSION_ID".into(), session.to_bits().to_string()),
        (
            "OZMUX_CONTROL_SOCK_PATH".into(),
            ext.control_sock_path().to_string_lossy().into_owned(),
        ),
    ]
}

fn drain_control_requests(ext: Res<ControlExtension>, mut mux: MultiplexerCommands) {
    while let Ok((req, responder)) = ext.0.control_requests().try_recv() {
        apply_control_request(&mut mux, req, responder);
    }
}

fn apply_control_request(mux: &mut MultiplexerCommands, req: ControlRequest, responder: Responder) {
    let resp = match resolve_and_split(mux, req) {
        Ok(reply) => ControlResponse::Ok(reply),
        Err(e) => ControlResponse::Err(e),
    };
    let _ = responder.send(resp);
}

fn resolve_and_split(
    mux: &mut MultiplexerCommands,
    req: ControlRequest,
) -> Result<SplitReply, ControlError> {
    let pane = Entity::try_from_bits(req.pane_bits)
        .filter(|e| mux.session_of_pane(*e).is_some())
        .ok_or_else(|| ControlError {
            code: "pane_not_found".into(),
            message: format!("no live pane for bits {}", req.pane_bits),
        })?;
    let ControlOp::Split(p) = req.op;
    let activity_id = p.activity.activity_id.clone();
    let kind = match p.activity.kind {
        ActivityKindSpec::Extension { html_root } => ActivityKind::Extension {
            html_root: PathBuf::from(html_root),
        },
    };
    let side = match p.side {
        ControlSide::Before => Side::Before,
        ControlSide::After => Side::After,
    };
    let orientation = match p.orientation {
        ControlOrientation::Horizontal => SplitOrientation::Horizontal,
        ControlOrientation::Vertical => SplitOrientation::Vertical,
    };
    let outcome = mux
        .split_pane_with_activity(pane, side, orientation, kind)
        .map_err(|e| ControlError {
            code: "internal".into(),
            message: e.to_string(),
        })?;
    mux.insert_on(outcome.activity, ExtensionActivityAid(activity_id));
    Ok(SplitReply {
        new_pane_id: outcome.pane.to_bits(),
        new_activity_id: outcome.activity.to_bits(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::bounded;
    use ozmux_multiplexer::{ActivityKind, ActivityMarker, MultiplexerCommands};

    fn split_request(pane_bits: u64) -> ControlRequest {
        ControlRequest {
            pane_bits,
            op: ControlOp::Split(crate::control::SplitParams {
                side: ControlSide::After,
                orientation: ControlOrientation::Vertical,
                activity: crate::control::ActivitySpec {
                    kind: ActivityKindSpec::Extension {
                        html_root: "/x/memo".into(),
                    },
                    name: None,
                    activity_id: "aid-xyz".into(),
                },
            }),
        }
    }

    #[test]
    fn handles_split_and_creates_extension_pane() {
        let mut world = World::new();
        let created = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let pane_bits = created.pane.to_bits();

        let (resp_tx, resp_rx) = bounded(1);
        // NOTE: wrap in Option so the closure is FnMut; the sender is consumed
        // on the first (and only) invocation.
        let mut resp_tx = Some(resp_tx);
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply_control_request(&mut mux, split_request(pane_bits), resp_tx.take().unwrap());
            })
            .unwrap();
        world.flush();

        match resp_rx.try_recv().unwrap() {
            ControlResponse::Ok(reply) => {
                let new_pane = Entity::try_from_bits(reply.new_pane_id).unwrap();
                assert!(
                    world
                        .get::<ozmux_multiplexer::PaneMarker>(new_pane)
                        .is_some()
                );
                let new_act = Entity::try_from_bits(reply.new_activity_id).unwrap();
                assert!(matches!(
                    world.get::<ActivityKind>(new_act),
                    Some(ActivityKind::Extension { .. })
                ));
                let aid = world.get::<ozmux_multiplexer::ExtensionActivityAid>(new_act);
                assert_eq!(aid.map(|a| a.0.as_str()), Some("aid-xyz"));
            }
            ControlResponse::Err(e) => panic!("expected Ok, got {}", e.code),
        }
        let mut q = world.query_filtered::<&ActivityKind, With<ActivityMarker>>();
        assert!(
            q.iter(&world)
                .any(|k| matches!(k, ActivityKind::Extension { .. }))
        );
    }

    #[test]
    fn unknown_pane_bits_yield_pane_not_found() {
        let mut world = World::new();
        world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let (resp_tx, resp_rx) = bounded(1);
        // NOTE: wrap in Option so the closure is FnMut; consumed on first call.
        let mut resp_tx = Some(resp_tx);
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply_control_request(
                    &mut mux,
                    split_request(999_999_999),
                    resp_tx.take().unwrap(),
                );
            })
            .unwrap();
        match resp_rx.try_recv().unwrap() {
            ControlResponse::Err(e) => assert_eq!(e.code, "pane_not_found"),
            ControlResponse::Ok(_) => panic!("expected pane_not_found"),
        }
    }
}
