//! Bevy glue: owns the launched `CommandExtension`, drains its control
//! requests each frame, resolves `OZMUX_PANE_ID` entity bits, and applies the
//! requested op via `MultiplexerCommands`. Depends on `bevy` + `ozmux_multiplexer`.

use crate::command::{CommandExtension, CommandExtensionConfig, Responder};
use crate::control::{
    ActivateParams, AddSurfaceParams, ControlError, ControlOp, ControlOrientation, ControlReply,
    ControlRequest, ControlResponse, ControlSide, SplitParams, SurfaceKindSpec,
};
use crate::path_prefix::extension_path_prefix;
use bevy::prelude::*;
use ozmux_multiplexer::{
    BrowserProfile, Cwd, ExtensionSurfaceId, MultiplexerCommands, OwningExtension, Side,
    SplitOrientation, SurfaceKind,
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
    /// inserts its `ActiveSurface` / `ChildOf` through the deferred `Commands`
    /// queue. A UI rebuild that reacts to `Changed<LayoutCells>` MUST run after
    /// this set so the inserted `ApplyDeferred` sync point flushes those
    /// commands first — otherwise the rebuild sees a pane with no surface yet
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
/// control requests reach this host: PATH (every extension's bin dir prefixed),
/// `OZMUX_PANE_ID`/`OZMUX_SESSION_ID` (entity bits), `OZMUX_CONTROL_SOCK_PATH`.
///
/// Every launched extension's `bin_dir` is prepended so any extension's
/// `@<cmd>` shim resolves from a terminal. Each shim file already encodes its
/// own extension's command socket (baked in at shim-write time from
/// `OZMUX_SOCK_PATH`), so a command always reaches the right extension's server
/// regardless of PATH order.
///
/// `OZMUX_CONTROL_SOCK_PATH` is a single value; it is set to the FIRST
/// extension's control socket. This is correct for any extension's split call:
/// the control bridge resolves the split purely from the request payload (the
/// owning extension name, entry, side/orientation come from the calling
/// extension's `callControl`), so applying a split through any extension's
/// control socket produces the same multiplexer mutation. The env carries no
/// per-extension control state — only a generic "reach the host" endpoint.
pub fn terminal_env(
    extensions: &[&CommandExtension],
    pane: Entity,
    workspace: Entity,
) -> Vec<(String, String)> {
    let current = std::env::var("PATH").unwrap_or_default();
    let bins: Vec<PathBuf> = extensions
        .iter()
        .map(|e| e.bin_dir().to_path_buf())
        .collect();
    let mut env = vec![
        ("PATH".into(), extension_path_prefix(&bins, &current)),
        ("OZMUX_PANE_ID".into(), pane.to_bits().to_string()),
        // NOTE: the env-var key keeps its legacy "SESSION" name on purpose — it is a
        // wire contract the SDK and user extensions read; renaming it breaks them.
        ("OZMUX_SESSION_ID".into(), workspace.to_bits().to_string()),
    ];
    if let Some(first) = extensions.first() {
        env.push((
            "OZMUX_CONTROL_SOCK_PATH".into(),
            first.control_sock_path().to_string_lossy().into_owned(),
        ));
    }
    env
}

fn drain_control_requests(ext: Res<ControlExtension>, mut mux: MultiplexerCommands) {
    while let Ok((req, responder)) = ext.0.control_requests().try_recv() {
        apply_control_request(&mut mux, req, responder);
    }
}

/// Resolves a control request against the multiplexer and replies to the
/// extension via `responder`. Shared by the single-extension
/// `drain_control_requests` and the multi-extension manager's drain, so every
/// extension's control socket applies ops through the same path.
pub fn apply_control_request(
    mux: &mut MultiplexerCommands,
    req: ControlRequest,
    responder: Responder,
) {
    let resp = match req.op {
        ControlOp::Split(p) => handle_split(mux, req.pane_bits, p),
        ControlOp::AddSurface(p) => handle_add_surface(mux, req.pane_bits, p),
        ControlOp::Activate(p) => handle_activate(mux, req.pane_bits, p),
    };
    let _ = responder.send(match resp {
        Ok(reply) => ControlResponse::Ok(reply),
        Err(e) => ControlResponse::Err(e),
    });
}

fn resolve_pane(mux: &MultiplexerCommands, pane_bits: u64) -> Result<Entity, ControlError> {
    Entity::try_from_bits(pane_bits)
        .filter(|e| mux.workspace_of_pane(*e).is_some())
        .ok_or_else(|| ControlError {
            code: "pane_not_found".into(),
            message: format!("no live pane for bits {pane_bits}"),
        })
}

fn stamp_extension_surface(
    mux: &mut MultiplexerCommands,
    surface: Entity,
    surface_id: String,
    extension_name: Option<String>,
) {
    mux.insert_on(surface, ExtensionSurfaceId(surface_id));
    if let Some(name) = extension_name {
        mux.insert_on(surface, OwningExtension(name));
    }
}

/// Maps a wire `SurfaceKindSpec` to the multiplexer `SurfaceKind`, returning
/// the optional owning-extension name and whether the kind is an extension
/// (only extension surfaces are stamped with `ExtensionSurfaceId` /
/// `OwningExtension`). Shared by `handle_split` and `handle_add_surface`.
fn surface_kind_from_spec(kind: SurfaceKindSpec) -> (SurfaceKind, Option<String>, bool) {
    match kind {
        SurfaceKindSpec::Extension {
            entry,
            extension_name,
        } => (
            SurfaceKind::Extension {
                entry: PathBuf::from(entry),
            },
            extension_name,
            true,
        ),
        SurfaceKindSpec::Browser { url } => (
            SurfaceKind::Browser {
                initial_url: Some(url),
                profile: BrowserProfile::default(),
            },
            None,
            false,
        ),
        SurfaceKindSpec::Terminal => (SurfaceKind::Terminal, None, false),
    }
}

fn handle_split(
    mux: &mut MultiplexerCommands,
    pane_bits: u64,
    p: SplitParams,
) -> Result<ControlReply, ControlError> {
    let pane = resolve_pane(mux, pane_bits)?;
    let surface_id = p.surface.surface_id.clone();
    let side = match p.side {
        ControlSide::Before => Side::Before,
        ControlSide::After => Side::After,
    };
    let orientation = match p.orientation {
        ControlOrientation::Horizontal => SplitOrientation::Horizontal,
        ControlOrientation::Vertical => SplitOrientation::Vertical,
    };
    let (kind, extension_name, is_extension) = surface_kind_from_spec(p.surface.kind);
    let outcome = mux
        .split_pane_with_surface(pane, side, orientation, kind)
        .map_err(|e| ControlError {
            code: "internal".into(),
            message: e.to_string(),
        })?;
    if is_extension {
        stamp_extension_surface(mux, outcome.surface, surface_id, extension_name);
    }
    if let Some(cwd) = p.surface.cwd {
        mux.insert_on(outcome.surface, Cwd(PathBuf::from(cwd)));
    }
    Ok(ControlReply::Split {
        new_pane_id: outcome.pane.to_bits(),
        new_surface_id: outcome.surface.to_bits(),
    })
}

fn handle_add_surface(
    mux: &mut MultiplexerCommands,
    pane_bits: u64,
    p: AddSurfaceParams,
) -> Result<ControlReply, ControlError> {
    let pane = resolve_pane(mux, pane_bits)?;
    let surface_id = p.surface.surface_id.clone();
    let (kind, extension_name, is_extension) = surface_kind_from_spec(p.surface.kind);
    let surface = mux.add_surface(pane, kind);
    if is_extension {
        stamp_extension_surface(mux, surface, surface_id, extension_name);
    }
    if let Some(cwd) = p.surface.cwd {
        mux.insert_on(surface, Cwd(PathBuf::from(cwd)));
    }
    Ok(ControlReply::AddSurface {
        new_surface_id: surface.to_bits(),
    })
}

fn handle_activate(
    mux: &mut MultiplexerCommands,
    pane_bits: u64,
    p: ActivateParams,
) -> Result<ControlReply, ControlError> {
    let pane = resolve_pane(mux, pane_bits)?;
    let surface = p
        .surface_id
        .parse::<u64>()
        .ok()
        .and_then(Entity::try_from_bits)
        .ok_or_else(|| ControlError {
            code: "bad_request".into(),
            message: format!("bad surface_id: {}", p.surface_id),
        })?;
    // Reject a surface that is not a live child of the invoking pane:
    // `set_active_surface` only validates the pane, so without this an
    // extension could point a pane's `ActiveSurface` at a foreign, stale, or
    // non-surface entity and corrupt its rendered state. `pane_of_surface`
    // returns `None` for despawned / recycled / non-surface bits.
    if mux.pane_of_surface(surface) != Some(pane) {
        return Err(ControlError {
            code: "bad_request".into(),
            message: format!("surface {} is not in the invoking pane", p.surface_id),
        });
    }
    mux.set_active_surface(pane, surface)
        .map_err(|e| ControlError {
            code: "internal".into(),
            message: e.to_string(),
        })?;
    Ok(ControlReply::Activate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::bounded;
    use ozmux_multiplexer::{
        MultiplexerCommands, SurfaceKind, SurfaceMarker, WorkspaceNameCounter,
    };

    fn split_request(pane_bits: u64) -> ControlRequest {
        ControlRequest {
            pane_bits,
            op: ControlOp::Split(crate::control::SplitParams {
                side: ControlSide::After,
                orientation: ControlOrientation::Vertical,
                surface: crate::control::SurfaceSpec {
                    kind: SurfaceKindSpec::Extension {
                        entry: "/x/memo".into(),
                        extension_name: Some("memo".into()),
                    },
                    name: None,
                    surface_id: "aid-xyz".into(),
                    cwd: None,
                },
            }),
        }
    }

    fn add_surface_request(pane_bits: u64) -> ControlRequest {
        ControlRequest {
            pane_bits,
            op: ControlOp::AddSurface(crate::control::AddSurfaceParams {
                surface: crate::control::SurfaceSpec {
                    kind: SurfaceKindSpec::Extension {
                        entry: "index.html".into(),
                        extension_name: Some("md".into()),
                    },
                    name: Some("x.md".into()),
                    surface_id: "aid-1".into(),
                    cwd: None,
                },
            }),
        }
    }

    fn browser_split_request(pane_bits: u64) -> ControlRequest {
        ControlRequest {
            pane_bits,
            op: ControlOp::Split(crate::control::SplitParams {
                side: ControlSide::After,
                orientation: ControlOrientation::Vertical,
                surface: crate::control::SurfaceSpec {
                    kind: SurfaceKindSpec::Browser {
                        url: "github.com".into(),
                    },
                    name: None,
                    surface_id: "aid-b".into(),
                    cwd: None,
                },
            }),
        }
    }

    #[test]
    fn handles_split_and_creates_browser_pane_without_extension_components() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let created = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let pane_bits = created.pane.to_bits();

        let (resp_tx, resp_rx) = bounded(1);
        let mut resp_tx = Some(resp_tx);
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply_control_request(
                    &mut mux,
                    browser_split_request(pane_bits),
                    resp_tx.take().unwrap(),
                );
            })
            .unwrap();
        world.flush();

        match resp_rx.try_recv().unwrap() {
            ControlResponse::Ok(ControlReply::Split { new_surface_id, .. }) => {
                let new_act = Entity::try_from_bits(new_surface_id).unwrap();
                match world.get::<SurfaceKind>(new_act) {
                    Some(SurfaceKind::Browser { initial_url, .. }) => {
                        assert_eq!(initial_url.as_deref(), Some("github.com"));
                    }
                    other => panic!("expected Browser kind, got {other:?}"),
                }
                assert!(
                    world
                        .get::<ozmux_multiplexer::ExtensionSurfaceId>(new_act)
                        .is_none(),
                    "browser surface must not get an ExtensionSurfaceId"
                );
                assert!(
                    world
                        .get::<ozmux_multiplexer::OwningExtension>(new_act)
                        .is_none(),
                    "browser surface must not get an OwningExtension"
                );
            }
            ControlResponse::Ok(_) => panic!("expected Split reply"),
            ControlResponse::Err(e) => panic!("expected Ok, got {}", e.code),
        }
    }

    #[test]
    fn handles_split_and_creates_extension_pane() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let created = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
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
            ControlResponse::Ok(ControlReply::Split {
                new_pane_id,
                new_surface_id,
            }) => {
                let new_pane = Entity::try_from_bits(new_pane_id).unwrap();
                assert!(
                    world
                        .get::<ozmux_multiplexer::PaneMarker>(new_pane)
                        .is_some()
                );
                let new_act = Entity::try_from_bits(new_surface_id).unwrap();
                assert!(matches!(
                    world.get::<SurfaceKind>(new_act),
                    Some(SurfaceKind::Extension { .. })
                ));
                let surface_id = world.get::<ozmux_multiplexer::ExtensionSurfaceId>(new_act);
                assert_eq!(surface_id.map(|a| a.0.as_str()), Some("aid-xyz"));
                let owner = world.get::<ozmux_multiplexer::OwningExtension>(new_act);
                assert_eq!(owner.map(|o| o.0.as_str()), Some("memo"));
            }
            ControlResponse::Ok(_) => panic!("expected Split reply"),
            ControlResponse::Err(e) => panic!("expected Ok, got {}", e.code),
        }
        let mut q = world.query_filtered::<&SurfaceKind, With<SurfaceMarker>>();
        assert!(
            q.iter(&world)
                .any(|k| matches!(k, SurfaceKind::Extension { .. }))
        );
    }

    #[test]
    fn unknown_pane_bits_yield_pane_not_found() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
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

    #[test]
    fn handles_add_surface_on_existing_pane() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let created = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let pane_bits = created.pane.to_bits();
        let (tx, rx) = bounded(1);
        let mut tx = Some(tx);
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply_control_request(&mut mux, add_surface_request(pane_bits), tx.take().unwrap());
            })
            .unwrap();
        world.flush();
        match rx.try_recv().unwrap() {
            ControlResponse::Ok(ControlReply::AddSurface { new_surface_id }) => {
                let act = Entity::try_from_bits(new_surface_id).unwrap();
                assert!(matches!(
                    world.get::<SurfaceKind>(act),
                    Some(SurfaceKind::Extension { .. })
                ));
                assert_eq!(
                    world
                        .get::<ozmux_multiplexer::ExtensionSurfaceId>(act)
                        .map(|a| a.0.as_str()),
                    Some("aid-1")
                );
                assert_eq!(
                    world.get::<ChildOf>(act).map(|c| c.parent()),
                    Some(created.pane)
                );
            }
            _ => panic!("expected AddSurface ok"),
        }
    }

    #[test]
    fn handles_activate_repoints_active_surface() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let created = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let pane = created.pane;
        let second = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(pane, SurfaceKind::Terminal)
            })
            .unwrap();
        world.flush();
        let (tx, rx) = bounded(1);
        let mut tx = Some(tx);
        let mut req = Some(ControlRequest {
            pane_bits: pane.to_bits(),
            op: ControlOp::Activate(crate::control::ActivateParams {
                surface_id: second.to_bits().to_string(),
            }),
        });
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply_control_request(&mut mux, req.take().unwrap(), tx.take().unwrap());
            })
            .unwrap();
        world.flush();
        assert!(matches!(
            rx.try_recv().unwrap(),
            ControlResponse::Ok(ControlReply::Activate)
        ));
        assert_eq!(
            world
                .get::<ozmux_multiplexer::ActiveSurface>(pane)
                .map(|a| a.0),
            Some(second)
        );
    }

    #[test]
    fn terminal_split_with_cwd_seeds_cwd_component() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let created = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let pane_bits = created.pane.to_bits();

        let req = ControlRequest {
            pane_bits,
            op: ControlOp::Split(crate::control::SplitParams {
                side: ControlSide::After,
                orientation: ControlOrientation::Vertical,
                surface: crate::control::SurfaceSpec {
                    kind: SurfaceKindSpec::Terminal,
                    name: Some("shell".into()),
                    surface_id: "aid-term".into(),
                    cwd: Some("/work".into()),
                },
            }),
        };
        let (tx, rx) = bounded(1);
        let mut tx = Some(tx);
        let mut req = Some(req);
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply_control_request(&mut mux, req.take().unwrap(), tx.take().unwrap());
            })
            .unwrap();
        world.flush();

        match rx.try_recv().unwrap() {
            ControlResponse::Ok(ControlReply::Split { new_surface_id, .. }) => {
                let surface = Entity::try_from_bits(new_surface_id).unwrap();
                assert!(
                    matches!(
                        world.get::<SurfaceKind>(surface),
                        Some(SurfaceKind::Terminal)
                    ),
                    "expected Terminal surface kind"
                );
                assert_eq!(
                    world.get::<Cwd>(surface),
                    Some(&Cwd(std::path::PathBuf::from("/work"))),
                    "expected Cwd to be seeded from the spec"
                );
                assert!(
                    world
                        .get::<ozmux_multiplexer::ExtensionSurfaceId>(surface)
                        .is_none(),
                    "terminal surface must not get an ExtensionSurfaceId"
                );
            }
            ControlResponse::Ok(_) => panic!("expected Split reply"),
            ControlResponse::Err(e) => panic!("expected Ok, got {}", e.code),
        }
    }

    #[test]
    fn browser_split_with_cwd_seeds_cwd_component() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let created = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let pane_bits = created.pane.to_bits();

        let req = ControlRequest {
            pane_bits,
            op: ControlOp::Split(crate::control::SplitParams {
                side: ControlSide::After,
                orientation: ControlOrientation::Vertical,
                surface: crate::control::SurfaceSpec {
                    kind: SurfaceKindSpec::Browser {
                        url: "github.com".into(),
                    },
                    name: None,
                    surface_id: "aid-b".into(),
                    cwd: Some("/work".into()),
                },
            }),
        };
        let (tx, rx) = bounded(1);
        let mut tx = Some(tx);
        let mut req = Some(req);
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply_control_request(&mut mux, req.take().unwrap(), tx.take().unwrap());
            })
            .unwrap();
        world.flush();

        match rx.try_recv().unwrap() {
            ControlResponse::Ok(ControlReply::Split { new_surface_id, .. }) => {
                let surface = Entity::try_from_bits(new_surface_id).unwrap();
                assert!(
                    matches!(
                        world.get::<SurfaceKind>(surface),
                        Some(SurfaceKind::Browser { .. })
                    ),
                    "expected Browser surface kind"
                );
                assert_eq!(
                    world.get::<Cwd>(surface),
                    Some(&Cwd(std::path::PathBuf::from("/work"))),
                    "browser surface must also seed Cwd from the spec"
                );
                assert!(
                    world
                        .get::<ozmux_multiplexer::ExtensionSurfaceId>(surface)
                        .is_none(),
                    "browser surface must not get an ExtensionSurfaceId"
                );
            }
            ControlResponse::Ok(_) => panic!("expected Split reply"),
            ControlResponse::Err(e) => panic!("expected Ok, got {}", e.code),
        }
    }

    #[test]
    fn activate_rejects_surface_not_in_pane() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let first = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let second = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let active_before = world
            .get::<ozmux_multiplexer::ActiveSurface>(first.pane)
            .map(|a| a.0);

        let (tx, rx) = bounded(1);
        let mut tx = Some(tx);
        // Try to activate workspace 2's surface on workspace 1's pane.
        let mut req = Some(ControlRequest {
            pane_bits: first.pane.to_bits(),
            op: ControlOp::Activate(crate::control::ActivateParams {
                surface_id: second.surface.to_bits().to_string(),
            }),
        });
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                apply_control_request(&mut mux, req.take().unwrap(), tx.take().unwrap());
            })
            .unwrap();
        world.flush();

        match rx.try_recv().unwrap() {
            ControlResponse::Err(e) => assert_eq!(e.code, "bad_request"),
            ControlResponse::Ok(_) => panic!("expected bad_request for a foreign surface"),
        }
        assert_eq!(
            world
                .get::<ozmux_multiplexer::ActiveSurface>(first.pane)
                .map(|a| a.0),
            active_before,
            "the foreign activate must not mutate the pane's ActiveSurface"
        );
    }
}
