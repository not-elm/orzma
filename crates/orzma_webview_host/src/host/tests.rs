//! Unit tests for the webview host, driving its entry points directly.

use super::*;
use crate::boundary::{ForwardChord, MountSpec, WebviewAsset};
use crate::error::WebviewHostError;
use crate::protocol::RegisterKind;
use crossbeam_channel::{Receiver, bounded, unbounded};
use serde_json::{Value, json};
use std::collections::HashMap;

const SOCK: &str = "/tmp/orzma-test/control.sock";
const SIZE: PlacementSize = PlacementSize { rows: 10, cols: 40 };

/// A host with an injected socket, driven directly, keeping every
/// connection's pushed lines.
struct Fixture {
    host: WebviewHost<u32>,
    lines: HashMap<u64, Receiver<String>>,
}

impl Fixture {
    fn new() -> Self {
        let (socket, _events) = ControlSocket::injected(SOCK);
        Self {
            host: WebviewHost::with_socket(socket),
            lines: HashMap::new(),
        }
    }

    /// Binds `pane` and returns its token.
    fn open_pane(&mut self, pane: u32) -> String {
        self.host
            .bind_pane(pane)
            .expect("a token mints")
            .into_iter()
            .find(|(key, _)| key == "ORZMA_TOKEN")
            .map(|(_, value)| value)
            .expect("the environment carries a token")
    }

    /// Sends a `hello` for `connection` with `token` and returns the answer.
    fn connect(&mut self, connection: u64, token: &str) -> bool {
        let (writer, lines) = unbounded();
        let (reply, answer) = bounded(1);
        let output = self
            .host
            .control(ControlEvent::Hello {
                connection: ConnectionId::new(connection),
                token: token.into(),
                writer,
                reply,
            })
            .expect("a hello never fails");
        assert_eq!(output, HostOutput::default());
        self.lines.insert(connection, lines);
        answer.try_recv().expect("one answer")
    }

    /// Every line pushed to `connection` since the last call, parsed.
    fn pushes(&self, connection: u64) -> Vec<Value> {
        self.lines[&connection]
            .try_iter()
            .map(|line| serde_json::from_str(&line).expect("a push is JSON"))
            .collect()
    }

    /// Opens `pane` and connects `connection` from it.
    fn connect_pane(&mut self, connection: u64, pane: u32) {
        let token = self.open_pane(pane);
        assert!(self.connect(connection, &token));
    }

    fn register(&mut self, connection: u64, kind: RegisterKind) -> (ServerMsg, HostOutput<u32>) {
        let (reply, answer) = bounded(1);
        let output = self
            .host
            .control(ControlEvent::Register {
                connection: ConnectionId::new(connection),
                registration: ValidatedRegistration::try_from(kind),
                reply,
            })
            .expect("a register never fails");
        (answer.try_recv().expect("one reply"), output)
    }

    /// Registers `kind` for `connection`, returning its handle and first
    /// instance.
    fn registered(&mut self, connection: u64, kind: RegisterKind) -> (HandleId, InstanceId) {
        match self.register(connection, kind).0 {
            ServerMsg::Registered {
                handle, instance, ..
            } => (handle, instance.parse().expect("a wire instance")),
            other => panic!("expected Registered, got {other:?}"),
        }
    }

    fn new_instance(&mut self, connection: u64, handle: &HandleId) -> ServerMsg {
        let (reply, answer) = bounded(1);
        let output = self
            .host
            .control(ControlEvent::NewInstance {
                connection: ConnectionId::new(connection),
                handle: handle.clone(),
                reply,
            })
            .expect("a new_instance never fails");
        assert_eq!(output, HostOutput::default());
        answer.try_recv().expect("one reply")
    }

    fn mounted(&mut self, pane: u32, instance: InstanceId) -> MountId {
        let output = self.host.placement_signal(
            pane,
            PlacementSignal::Mounted {
                instance,
                size: SIZE,
            },
        );
        match output.events() {
            [WebviewEvent::Mounted { mount, .. }] => *mount,
            other => panic!("expected one Mounted, got {other:?}"),
        }
    }

    fn control(&mut self, event: ControlEvent) -> WebviewHostResult<HostOutput<u32>> {
        self.host.control(event)
    }
}

fn inline() -> RegisterKind {
    RegisterKind::Inline {
        html: "<h1>x</h1>".into(),
        interactive: true,
        forward_keys: vec![],
        preload: vec![],
    }
}

fn url(bridge: bool) -> RegisterKind {
    RegisterKind::Url {
        url: "https://example.com/".into(),
        interactive: true,
        bridge,
        forward_keys: vec![],
        preload: vec![],
    }
}

fn connection(raw: u64) -> ConnectionId {
    ConnectionId::new(raw)
}

fn refusal(result: WebviewHostResult<HostOutput<u32>>) -> Refusal {
    match result {
        Err(WebviewHostError::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn focus_changed(handle: &HandleId, instance: InstanceId, focused: bool) -> Value {
    json!({"op": "focus_changed", "handle": handle.as_str(), "instance": instance.to_string(), "focused": focused})
}

fn gui_focus(fixture: &mut Fixture, mount: Option<MountId>) -> HostOutput<u32> {
    fixture
        .host
        .command(WebviewCommand::Focus { mount })
        .expect("a GUI focus never fails")
}

/// A fixture with pane 1 active, connection 1 in it, and one mounted
/// inline view; returns the handle, instance, and mount.
fn focused_fixture() -> (Fixture, HandleId, InstanceId, MountId) {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let _ = fixture.host.active_pane_changed(Some(1));
    let (handle, instance) = fixture.registered(1, inline());
    let mount = fixture.mounted(1, instance);
    (fixture, handle, instance, mount)
}

/// Asserts that a bound pane's environment carries the socket path and a
/// token of its own.
///
/// Case: the user opens two panes and each shell starts with its control
/// variables.
#[test]
fn binding_a_pane_hands_it_the_socket_and_a_token() {
    let mut fixture = Fixture::new();
    let env = fixture.host.bind_pane(1).expect("a token mints");
    assert_eq!(env[0], ("ORZMA_SOCK".to_string(), SOCK.to_string()));
    assert_eq!(env[1].0, "ORZMA_TOKEN");
    assert!(env[1].1.starts_with("orzma:"));
    let other = fixture.open_pane(2);
    assert_ne!(env[1].1, other);
}

/// Asserts that a host without a socket gives panes no environment and has
/// no control channel.
///
/// Case: the control socket failed to bind at startup and the user opens a
/// pane anyway.
#[test]
fn a_host_without_a_socket_hands_panes_no_environment() {
    let mut host = WebviewHost::<u32>::without_socket();
    assert!(host.bind_pane(1).expect("never fails").is_empty());
    assert!(host.control_events().is_none());
    assert!(host.try_recv_control().is_none());
}

/// Asserts that a `hello` with a bound token is accepted, and one with an
/// unknown token or the token of a closed pane is refused.
///
/// Case: a pane's shell connects, a stray process guesses a token, and a
/// leftover process of a closed pane reconnects.
#[test]
fn a_hello_is_accepted_only_for_a_live_panes_token() {
    let mut fixture = Fixture::new();
    let token = fixture.open_pane(1);
    assert!(fixture.connect(1, &token));
    assert!(!fixture.connect(2, "orzma:guess"));
    let _ = fixture.host.pane_closed(1);
    assert!(!fixture.connect(3, &token));
}

/// Asserts that an inline registration replies with its handle and first
/// instance, and has its document served.
///
/// Case: a markdown viewer registers its rendered page inline.
#[test]
fn an_inline_register_serves_its_document() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (reply, output) = fixture.register(1, inline());
    let ServerMsg::Registered { handle, .. } = reply else {
        panic!("expected Registered, got {reply:?}");
    };
    assert_eq!(
        output.events(),
        [WebviewEvent::AssetRegistered {
            handle,
            asset: WebviewAsset::Inline(b"<h1>x</h1>".to_vec()),
        }]
    );
}

/// Asserts that a directory registration has its root served and a remote
/// URL registration has nothing served.
///
/// Case: one program registers its bundle directory and another a remote
/// page.
#[test]
fn a_dir_register_serves_its_root_and_a_url_register_nothing() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let root = tempfile::tempdir().unwrap();
    let (_, output) = fixture.register(
        1,
        RegisterKind::Dir {
            root: root.path().to_str().unwrap().into(),
            entry: "index.html".into(),
            interactive: true,
            forward_keys: vec![],
            preload: vec![],
        },
    );
    assert!(matches!(
        output.events(),
        [WebviewEvent::AssetRegistered { asset: WebviewAsset::Dir(path), .. }] if path.as_path() == root.path()
    ));
    let (reply, output) = fixture.register(1, url(false));
    assert!(matches!(reply, ServerMsg::Registered { .. }));
    assert!(output.events().is_empty());
}

/// Asserts that a register from a closed pane replies `owner_gone` even when
/// the payload is also invalid, and registers nothing.
///
/// Case: a program keeps its connection open after its pane was killed and
/// sends another `register`.
#[test]
fn a_register_from_a_closed_pane_replies_owner_gone() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let _ = fixture.host.pane_closed(1);
    let (reply, output) = fixture.register(1, inline());
    assert_eq!(reply, ServerMsg::err("owner_gone"));
    assert_eq!(output, HostOutput::default());
    let (reply, _) = fixture.register(
        1,
        RegisterKind::Dir {
            root: "relative".into(),
            entry: "index.html".into(),
            interactive: true,
            forward_keys: vec![],
            preload: vec![],
        },
    );
    assert_eq!(reply, ServerMsg::err("owner_gone"));
}

/// Asserts that an invalid payload from a live pane replies with its
/// validation code.
///
/// Case: a program registers a relative root directory.
#[test]
fn an_invalid_register_replies_its_validation_code() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (reply, output) = fixture.register(
        1,
        RegisterKind::Dir {
            root: "relative".into(),
            entry: "index.html".into(),
            interactive: true,
            forward_keys: vec![],
            preload: vec![],
        },
    );
    assert_eq!(reply, ServerMsg::err("invalid_root"));
    assert_eq!(output, HostOutput::default());
}

/// Asserts that `new_instance` mints a fresh instance for the owner and
/// refuses another connection and an unknown handle.
///
/// Case: a program asks for a second placement of its view, another program
/// tries the same handle, and a third names a stale handle.
#[test]
fn new_instance_serves_only_the_owner() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    fixture.connect_pane(2, 2);
    let (handle, first) = fixture.registered(1, inline());
    let ServerMsg::Instanced { instance, .. } = fixture.new_instance(1, &handle) else {
        panic!("expected Instanced");
    };
    assert_ne!(instance.parse::<InstanceId>().unwrap(), first);
    assert_eq!(
        fixture.new_instance(2, &handle),
        ServerMsg::err("not_owner")
    );
    assert_eq!(
        fixture.new_instance(1, &HandleId::from("missing")),
        ServerMsg::err("unknown_handle")
    );
}

/// Asserts that `unregister` drops the reservations of every minted
/// instance and stops serving the asset, and that another connection's
/// `unregister` is refused.
///
/// Case: a program holding two placements of one view unregisters it after
/// another program tried to.
#[test]
fn unregister_releases_reservations_and_the_asset() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    fixture.connect_pane(2, 2);
    let (handle, first) = fixture.registered(1, inline());
    let ServerMsg::Instanced { instance, .. } = fixture.new_instance(1, &handle) else {
        panic!("expected Instanced");
    };
    let second: InstanceId = instance.parse().unwrap();
    assert_eq!(
        refusal(fixture.control(ControlEvent::Unregister {
            connection: connection(2),
            handle: handle.clone(),
        })),
        Refusal::NotOwner
    );
    let output = fixture
        .control(ControlEvent::Unregister {
            connection: connection(1),
            handle: handle.clone(),
        })
        .expect("the owner unregisters");
    assert_eq!(
        output.requests(),
        [MuxRequest::RemovePlacements {
            pane: 1,
            instances: vec![first, second],
        }]
    );
    assert_eq!(output.events(), [WebviewEvent::AssetReleased { handle }]);
}

/// Asserts that a disconnect releases every registration of that connection
/// and none of another's.
///
/// Case: a program exits while another program in another pane keeps its
/// view.
#[test]
fn disconnect_releases_only_that_connections_registrations() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    fixture.connect_pane(2, 2);
    let (a, _) = fixture.registered(1, inline());
    let (b, _) = fixture.registered(1, url(false));
    let (c, _) = fixture.registered(2, inline());
    let output = fixture
        .control(ControlEvent::Disconnect {
            connection: connection(1),
        })
        .expect("a disconnect never fails");
    assert_eq!(output.requests().len(), 2);
    assert_eq!(output.events(), [WebviewEvent::AssetReleased { handle: a }]);
    assert_eq!(
        fixture.new_instance(2, &b),
        ServerMsg::err("unknown_handle")
    );
    assert!(matches!(
        fixture.new_instance(2, &c),
        ServerMsg::Instanced { .. }
    ));
}

/// Asserts that a VT mount of an owned instance reports a new mount carrying
/// the spec its registration implies.
///
/// Case: a markdown viewer writes its APC mount and the VT accepts it.
#[test]
fn a_mount_reports_a_new_mount_with_its_spec() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (handle, instance) = fixture.registered(1, inline());
    let output = fixture.host.placement_signal(
        1,
        PlacementSignal::Mounted {
            instance,
            size: SIZE,
        },
    );
    let [
        WebviewEvent::Mounted {
            pane,
            mount,
            instance: mounted,
            spec,
        },
    ] = output.events()
    else {
        panic!("expected one Mounted, got {:?}", output.events());
    };
    assert_eq!((*pane, *mounted), (1, instance));
    assert_eq!(*mount, MountId::new(1));
    assert_eq!(
        *spec,
        MountSpec::new(handle.clone(), format!("orzma://{handle}/index.html"), SIZE)
            .with_bridge(true)
    );
    assert!(output.requests().is_empty());
}

/// Asserts that a mount's spec carries the registration's input policy,
/// bridge choice, preload scripts, and forward keys.
///
/// Case: a program registers a read-only remote page with a preload script
/// and a forward key, and mounts it.
#[test]
fn a_mount_spec_carries_the_registration_policy() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let chord = ForwardChord::new(vec!["alt".into()], "h");
    let (_, instance) = fixture.registered(
        1,
        RegisterKind::Url {
            url: "https://example.com/".into(),
            interactive: false,
            bridge: false,
            forward_keys: vec![chord.clone()],
            preload: vec!["window.A = 1;".into()],
        },
    );
    let output = fixture.host.placement_signal(
        1,
        PlacementSignal::Mounted {
            instance,
            size: SIZE,
        },
    );
    let [WebviewEvent::Mounted { spec, .. }] = output.events() else {
        panic!("expected one Mounted");
    };
    assert_eq!(spec.url(), "https://example.com/");
    assert!(!spec.interactive());
    assert!(!spec.bridged());
    assert_eq!(spec.preload(), ["window.A = 1;".to_string()]);
    assert_eq!(spec.forward_keys(), [chord]);
}

/// Asserts that a mount from a pane that does not own the instance, or of an
/// unknown instance, is answered by dropping the reservation.
///
/// Case: a program in one pane echoes another pane's instance into its own
/// terminal, and a stale instance from a previous session is mounted.
#[test]
fn a_foreign_or_unknown_mount_is_reclaimed() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (_, instance) = fixture.registered(1, inline());
    let foreign = fixture.host.placement_signal(
        2,
        PlacementSignal::Mounted {
            instance,
            size: SIZE,
        },
    );
    assert!(foreign.events().is_empty());
    assert_eq!(
        foreign.requests(),
        [MuxRequest::RemovePlacements {
            pane: 2,
            instances: vec![instance],
        }]
    );
    let unknown = fixture.host.placement_signal(
        1,
        PlacementSignal::Mounted {
            instance: InstanceId(77),
            size: SIZE,
        },
    );
    assert_eq!(
        unknown.requests(),
        [MuxRequest::RemovePlacements {
            pane: 1,
            instances: vec![InstanceId(77)],
        }]
    );
}

/// Asserts that re-mounting a mounted placement keeps its mount, reporting
/// only a size change.
///
/// Case: the SDK re-sends its mount on every redraw, and once after the
/// window grows.
#[test]
fn a_remount_reports_only_a_size_change() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (_, instance) = fixture.registered(1, inline());
    let mount = fixture.mounted(1, instance);
    let same = fixture.host.placement_signal(
        1,
        PlacementSignal::Mounted {
            instance,
            size: SIZE,
        },
    );
    assert_eq!(same, HostOutput::default());
    let larger = PlacementSize { rows: 12, cols: 48 };
    let grown = fixture.host.placement_signal(
        1,
        PlacementSignal::Mounted {
            instance,
            size: larger,
        },
    );
    assert_eq!(
        grown.events(),
        [WebviewEvent::Resized {
            mount,
            size: larger
        }]
    );
}

/// Asserts that an unmount ends the mount, and a later mount of the same
/// instance starts a new one.
///
/// Case: the program leaves and re-enters the alternate screen, unmounting
/// and re-mounting its placement.
#[test]
fn an_unmount_then_mount_starts_a_new_mount() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (_, instance) = fixture.registered(1, inline());
    let first = fixture.mounted(1, instance);
    let unmounted = fixture.host.placement_signal(
        1,
        PlacementSignal::Unmounted {
            instance: Some(instance),
        },
    );
    assert_eq!(
        unmounted.events(),
        [WebviewEvent::Unmounted {
            mounts: vec![first]
        }]
    );
    let second = fixture.mounted(1, instance);
    assert_ne!(first, second);
}

/// Asserts that an unmount of every placement ends exactly the mounts of
/// that pane, and that an eviction ends the named mounts and ignores
/// unknown ids.
///
/// Case: a program resets its terminal while another pane keeps its page,
/// and later the VT trims history past a placement.
#[test]
fn unmount_all_and_eviction_end_only_what_they_name() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    fixture.connect_pane(2, 2);
    let (handle, a) = fixture.registered(1, inline());
    let ServerMsg::Instanced { instance, .. } = fixture.new_instance(1, &handle) else {
        panic!("expected Instanced");
    };
    let b: InstanceId = instance.parse().unwrap();
    let (_, c) = fixture.registered(2, inline());
    let mount_a = fixture.mounted(1, a);
    let mount_b = fixture.mounted(1, b);
    let mount_c = fixture.mounted(2, c);
    let all = fixture
        .host
        .placement_signal(1, PlacementSignal::Unmounted { instance: None });
    let [WebviewEvent::Unmounted { mounts }] = all.events() else {
        panic!("expected one Unmounted");
    };
    let mut mounts = mounts.clone();
    mounts.sort();
    assert_eq!(mounts, [mount_a, mount_b]);
    let evicted = fixture.host.placement_signal(
        2,
        PlacementSignal::Evicted {
            instances: vec![c, InstanceId(99)],
        },
    );
    assert_eq!(
        evicted.events(),
        [WebviewEvent::Unmounted {
            mounts: vec![mount_c]
        }]
    );
}

/// Asserts that a mount the VT refused changes nothing.
///
/// Case: a program mounts a thirteenth placement in one pane.
#[test]
fn a_rejected_mount_changes_nothing() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (_, instance) = fixture.registered(1, inline());
    assert_eq!(
        fixture
            .host
            .placement_signal(1, PlacementSignal::Rejected { instance }),
        HostOutput::default()
    );
}

/// Asserts that a socket mount of an owned instance asks the multiplexer to
/// mount it at the given cell.
///
/// Case: orzmd in a Windows pane mounts its view over the socket.
#[test]
fn a_socket_mount_asks_the_multiplexer_to_mount() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (_, instance) = fixture.registered(1, inline());
    let output = fixture
        .control(ControlEvent::Mount {
            connection: connection(1),
            instance: instance.to_string(),
            row: 2,
            col: 3,
            rows: 12,
            cols: 48,
        })
        .expect("an owned mount is accepted");
    assert_eq!(
        output.requests(),
        [MuxRequest::MountPlacement {
            pane: 1,
            instance,
            row: ScreenLine(2),
            column: GridColumn(3),
            size: PlacementSize { rows: 12, cols: 48 },
        }]
    );
}

/// Asserts that a socket mount with a zero or oversized axis is refused.
///
/// Case: a buggy program computes its rect from a zero-sized window, and
/// another asks for more rows than the VT supports.
#[test]
fn a_socket_mount_outside_the_size_range_is_refused() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (_, instance) = fixture.registered(1, inline());
    for (rows, cols) in [(0, 10), (MAX_ROWS + 1, 10), (10, 0), (10, MAX_COLS + 1)] {
        let result = fixture.control(ControlEvent::Mount {
            connection: connection(1),
            instance: instance.to_string(),
            row: 0,
            col: 0,
            rows,
            cols,
        });
        assert_eq!(refusal(result), Refusal::SizeOutOfRange, "{rows}x{cols}");
    }
}

/// Asserts that socket requests naming another connection's instance, an
/// unknown instance, or a malformed one are refused.
///
/// Case: one program tries to unmount another program's page, a stale id,
/// and a truncated id.
#[test]
fn socket_requests_for_foreign_unknown_or_malformed_instances_are_refused() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    fixture.connect_pane(2, 2);
    let (_, instance) = fixture.registered(1, inline());
    let unmount = |spelled: String, from: u64| ControlEvent::Unmount {
        connection: connection(from),
        instance: spelled,
    };
    assert_eq!(
        refusal(fixture.control(unmount(instance.to_string(), 2))),
        Refusal::NotOwner
    );
    assert_eq!(
        refusal(fixture.control(unmount(InstanceId(5).to_string(), 1))),
        Refusal::UnknownInstance
    );
    assert_eq!(
        refusal(fixture.control(unmount("3f5a".into(), 1))),
        Refusal::MalformedInstance
    );
}

/// Asserts that a socket unmount ends the mount and drops the reservation.
///
/// Case: orzmd in a Windows pane closes its view over the socket.
#[test]
fn a_socket_unmount_ends_the_mount_and_drops_the_reservation() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (_, instance) = fixture.registered(1, inline());
    let mount = fixture.mounted(1, instance);
    let output = fixture
        .control(ControlEvent::Unmount {
            connection: connection(1),
            instance: instance.to_string(),
        })
        .expect("an owned unmount is accepted");
    assert_eq!(
        output.events(),
        [WebviewEvent::Unmounted {
            mounts: vec![mount]
        }]
    );
    assert_eq!(
        output.requests(),
        [MuxRequest::RemovePlacements {
            pane: 1,
            instances: vec![instance],
        }]
    );
}

/// Asserts that unregistering a mounted view ends its mount before it stops
/// serving the asset.
///
/// Case: a program unregisters its view while the page is on screen.
#[test]
fn unregistering_a_mounted_view_ends_its_mount() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (handle, instance) = fixture.registered(1, inline());
    let mount = fixture.mounted(1, instance);
    let output = fixture
        .control(ControlEvent::Unregister {
            connection: connection(1),
            handle: handle.clone(),
        })
        .expect("the owner unregisters");
    assert_eq!(
        output.events(),
        [
            WebviewEvent::Unmounted {
                mounts: vec![mount]
            },
            WebviewEvent::AssetReleased { handle },
        ]
    );
}

/// Asserts that closing a pane ends its mounts and stops serving its assets
/// without asking the multiplexer for anything.
///
/// Case: the user closes a pane whose program shows a page.
#[test]
fn closing_a_pane_ends_its_mounts_without_touching_the_vt() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (handle, instance) = fixture.registered(1, inline());
    let mount = fixture.mounted(1, instance);
    let output = fixture.host.pane_closed(1);
    assert_eq!(
        output.events(),
        [
            WebviewEvent::Unmounted {
                mounts: vec![mount]
            },
            WebviewEvent::AssetReleased { handle },
        ]
    );
    assert!(output.requests().is_empty());
}

/// Asserts that a `new_instance` for a handle of a closed pane answers
/// `unknown_handle` rather than `owner_gone`.
///
/// Case: a program keeps its connection open after its pane closes and asks
/// for another placement of the page it had registered.
#[test]
fn new_instance_after_the_pane_closes_answers_unknown_handle() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let (handle, _) = fixture.registered(1, inline());
    let _ = fixture.host.pane_closed(1);
    assert_eq!(
        fixture.new_instance(1, &handle),
        ServerMsg::err("unknown_handle")
    );
}

/// Asserts that only the four webview signals convert to placement signals,
/// and every other VT signal comes back unchanged.
///
/// Case: the multiplexer sorts one pump's VT signals into placement changes
/// and signals for the GUI.
#[test]
fn only_webview_vt_signals_are_placement_signals() {
    assert_eq!(
        PlacementSignal::try_from(VtSignal::WebviewMount {
            instance: InstanceId(1),
            size: SIZE,
        }),
        Ok(PlacementSignal::Mounted {
            instance: InstanceId(1),
            size: SIZE,
        })
    );
    assert_eq!(
        PlacementSignal::try_from(VtSignal::WebviewEvicted {
            placements: vec![InstanceId(2)],
        }),
        Ok(PlacementSignal::Evicted {
            instances: vec![InstanceId(2)],
        })
    );
    assert!(matches!(
        PlacementSignal::try_from(VtSignal::Bell),
        Err(VtSignal::Bell)
    ));
}

/// Asserts that a GUI focus on an interactive mount moves focus, tells the
/// owner `focused: true`, and answers with the new focus.
///
/// Case: the user clicks the page a markdown viewer mounted in the active
/// pane.
#[test]
fn a_gui_focus_moves_focus_and_tells_the_owner() {
    let (mut fixture, handle, instance, mount) = focused_fixture();
    let output = gui_focus(&mut fixture, Some(mount));
    assert_eq!(
        output.events(),
        [WebviewEvent::FocusChanged {
            focused: Some(mount)
        }]
    );
    assert!(output.requests().is_empty());
    assert_eq!(fixture.pushes(1), [focus_changed(&handle, instance, true)]);
}

/// Asserts that focusing a mount in an inactive pane asks the multiplexer to
/// select that pane.
///
/// Case: the user clicks a page mounted in the inactive right-hand pane.
#[test]
fn a_gui_focus_in_an_inactive_pane_selects_that_pane() {
    let (mut fixture, _, _, mount) = focused_fixture();
    let _ = fixture.host.active_pane_changed(Some(2));
    let output = gui_focus(&mut fixture, Some(mount));
    assert_eq!(output.requests(), [MuxRequest::SelectPane { pane: 1 }]);
}

/// Asserts that a GUI focus on a non-interactive or ended mount changes
/// nothing, pushes nothing, and still answers with the current focus.
///
/// Case: the user clicks a status badge that takes no input, and a page
/// that was unmounted in the same frame.
#[test]
fn a_refused_gui_focus_still_answers_with_the_current_focus() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let _ = fixture.host.active_pane_changed(Some(1));
    let (_, inert) = fixture.registered(
        1,
        RegisterKind::Inline {
            html: "<p>badge</p>".into(),
            interactive: false,
            forward_keys: vec![],
            preload: vec![],
        },
    );
    let inert_mount = fixture.mounted(1, inert);
    let output = gui_focus(&mut fixture, Some(inert_mount));
    assert_eq!(
        output.events(),
        [WebviewEvent::FocusChanged { focused: None }]
    );
    assert!(fixture.pushes(1).is_empty());
    let output = gui_focus(&mut fixture, Some(MountId::new(999)));
    assert_eq!(
        output.events(),
        [WebviewEvent::FocusChanged { focused: None }]
    );
}

/// Asserts that a GUI focus naming the mount a placement had before it was
/// unmounted and mounted again focuses nothing and pushes nothing.
///
/// Case: the user clicks a page at the moment its program re-mounts it, and
/// the click names the mount that just ended.
#[test]
fn a_gui_focus_of_a_replaced_mount_focuses_nothing() {
    let (mut fixture, _, instance, old) = focused_fixture();
    let _ = fixture.host.placement_signal(
        1,
        PlacementSignal::Unmounted {
            instance: Some(instance),
        },
    );
    let new = fixture.mounted(1, instance);
    assert_ne!(old, new);
    let output = gui_focus(&mut fixture, Some(old));
    assert_eq!(
        output.events(),
        [WebviewEvent::FocusChanged { focused: None }]
    );
    assert!(fixture.pushes(1).is_empty());
}

/// Asserts that a GUI blur releases focus and tells the owner
/// `focused: false`.
///
/// Case: the user presses the release-focus shortcut while a page holds
/// the keyboard.
#[test]
fn a_gui_blur_releases_focus() {
    let (mut fixture, handle, instance, mount) = focused_fixture();
    let _ = gui_focus(&mut fixture, Some(mount));
    let _ = fixture.pushes(1);
    let output = gui_focus(&mut fixture, None);
    assert_eq!(
        output.events(),
        [WebviewEvent::FocusChanged { focused: None }]
    );
    assert_eq!(fixture.pushes(1), [focus_changed(&handle, instance, false)]);
}

/// Asserts that focusing the mount that already holds focus answers but
/// pushes nothing.
///
/// Case: the user clicks the page that already holds focus.
#[test]
fn refocusing_the_same_mount_pushes_nothing() {
    let (mut fixture, _, _, mount) = focused_fixture();
    let _ = gui_focus(&mut fixture, Some(mount));
    let _ = fixture.pushes(1);
    let output = gui_focus(&mut fixture, Some(mount));
    assert_eq!(
        output.events(),
        [WebviewEvent::FocusChanged {
            focused: Some(mount)
        }]
    );
    assert!(fixture.pushes(1).is_empty());
}

/// Asserts that moving focus between two mounts pushes `false` for the old
/// one before `true` for the new one.
///
/// Case: the user clicks from one placement of a program to its second
/// placement.
#[test]
fn moving_focus_pushes_false_before_true() {
    let (mut fixture, handle, first, first_mount) = focused_fixture();
    let ServerMsg::Instanced { instance, .. } = fixture.new_instance(1, &handle) else {
        panic!("expected Instanced");
    };
    let second: InstanceId = instance.parse().unwrap();
    let second_mount = fixture.mounted(1, second);
    let _ = gui_focus(&mut fixture, Some(first_mount));
    let _ = fixture.pushes(1);
    let _ = gui_focus(&mut fixture, Some(second_mount));
    assert_eq!(
        fixture.pushes(1),
        [
            focus_changed(&handle, first, false),
            focus_changed(&handle, second, true),
        ]
    );
}

/// Asserts that a socket focus on an owned mounted placement moves focus
/// and reports it, and selects its pane when that pane is inactive.
///
/// Case: a program focuses its page when its TUI hands over the keyboard,
/// while the user works in another pane.
#[test]
fn a_socket_focus_moves_focus_to_an_owned_mount() {
    let (mut fixture, handle, instance, mount) = focused_fixture();
    let _ = fixture.host.active_pane_changed(Some(2));
    let output = fixture
        .control(ControlEvent::Focus {
            connection: connection(1),
            instance: Some(instance.to_string()),
        })
        .expect("an owned focus is accepted");
    assert_eq!(
        output.events(),
        [WebviewEvent::FocusChanged {
            focused: Some(mount)
        }]
    );
    assert_eq!(output.requests(), [MuxRequest::SelectPane { pane: 1 }]);
    assert_eq!(fixture.pushes(1), [focus_changed(&handle, instance, true)]);
}

/// Asserts that a socket focus on another connection's placement, an
/// unmounted placement, or a non-interactive one is refused.
///
/// Case: one program tries to steal focus for another program's page, a
/// program focuses a placement it never mounted, and another focuses its
/// status badge.
#[test]
fn a_socket_focus_on_a_foreign_unmounted_or_inert_placement_is_refused() {
    let (mut fixture, handle, instance, _) = focused_fixture();
    fixture.connect_pane(2, 2);
    let focus = |from: u64, instance: InstanceId| ControlEvent::Focus {
        connection: connection(from),
        instance: Some(instance.to_string()),
    };
    assert_eq!(
        refusal(fixture.control(focus(2, instance))),
        Refusal::NotOwner
    );
    let ServerMsg::Instanced {
        instance: spare, ..
    } = fixture.new_instance(1, &handle)
    else {
        panic!("expected Instanced");
    };
    let spare: InstanceId = spare.parse().unwrap();
    assert_eq!(
        refusal(fixture.control(focus(1, spare))),
        Refusal::NotMounted
    );
    let (_, inert) = fixture.registered(
        1,
        RegisterKind::Inline {
            html: "<p>badge</p>".into(),
            interactive: false,
            forward_keys: vec![],
            preload: vec![],
        },
    );
    let _ = fixture.mounted(1, inert);
    assert_eq!(
        refusal(fixture.control(focus(1, inert))),
        Refusal::NotInteractive
    );
}

/// Asserts that a socket blur releases focus only when it is held in the
/// connection's own pane.
///
/// Case: a program starts its search line and blurs, while the focused page
/// belongs to a program in another pane, which later blurs too.
#[test]
fn a_socket_blur_releases_only_its_own_panes_focus() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    fixture.connect_pane(2, 2);
    let _ = fixture.host.active_pane_changed(Some(2));
    let (handle, instance) = fixture.registered(2, inline());
    let mount = fixture.mounted(2, instance);
    let _ = gui_focus(&mut fixture, Some(mount));
    let _ = fixture.pushes(2);
    let blur = |from: u64| ControlEvent::Focus {
        connection: connection(from),
        instance: None,
    };
    let other_pane = fixture.control(blur(1)).expect("a blur is accepted");
    assert_eq!(other_pane, HostOutput::default());
    let own_pane = fixture.control(blur(2)).expect("a blur is accepted");
    assert_eq!(
        own_pane.events(),
        [WebviewEvent::FocusChanged { focused: None }]
    );
    assert_eq!(fixture.pushes(2), [focus_changed(&handle, instance, false)]);
}

/// Asserts that an active-pane change releases a focus held in another
/// pane, and keeps one held in the new active pane.
///
/// Case: the user moves to another pane with a directional shortcut while a
/// page holds focus, after first selecting the page's own pane again.
#[test]
fn an_active_pane_change_releases_focus_held_elsewhere() {
    let (mut fixture, handle, instance, mount) = focused_fixture();
    let _ = gui_focus(&mut fixture, Some(mount));
    let _ = fixture.pushes(1);
    assert_eq!(
        fixture.host.active_pane_changed(Some(1)),
        HostOutput::default()
    );
    let output = fixture.host.active_pane_changed(Some(2));
    assert_eq!(
        output.events(),
        [WebviewEvent::FocusChanged { focused: None }]
    );
    assert_eq!(fixture.pushes(1), [focus_changed(&handle, instance, false)]);
}

/// Asserts that ending the focused mount releases focus first, telling the
/// owner, and then reports the unmount.
///
/// Case: the program unmounts its page while the user is typing into it.
#[test]
fn ending_the_focused_mount_releases_focus_first() {
    let (mut fixture, handle, instance, mount) = focused_fixture();
    let _ = gui_focus(&mut fixture, Some(mount));
    let _ = fixture.pushes(1);
    let output = fixture.host.placement_signal(
        1,
        PlacementSignal::Unmounted {
            instance: Some(instance),
        },
    );
    assert_eq!(
        output.events(),
        [
            WebviewEvent::FocusChanged { focused: None },
            WebviewEvent::Unmounted {
                mounts: vec![mount]
            },
        ]
    );
    assert_eq!(fixture.pushes(1), [focus_changed(&handle, instance, false)]);
}

/// Asserts that a display-only remote page, which has no back-channel,
/// still gets focus pushes.
///
/// Case: a program mounts a remote page without the bridge and the user
/// clicks it.
#[test]
fn a_display_only_url_view_gets_focus_pushes() {
    let mut fixture = Fixture::new();
    fixture.connect_pane(1, 1);
    let _ = fixture.host.active_pane_changed(Some(1));
    let (handle, instance) = fixture.registered(1, url(false));
    let mount = fixture.mounted(1, instance);
    let _ = gui_focus(&mut fixture, Some(mount));
    assert_eq!(fixture.pushes(1), [focus_changed(&handle, instance, true)]);
}
