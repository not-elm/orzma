//! Tests of the webview host running inside the multiplexer loop.

use crate::backend::{NewPaneAt, OrzmuxEvent, PaneId, PaneTarget, RequestId, SplitOrientation};
use crate::event_loop::OrzmuxCommand;
use crate::test_support::{CONTROL_SOCK, FakePane, Harness};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use orzma_vt::prelude::{InstanceId, MAX_PLACEMENTS, PlacementSize};
use orzma_webview_host::prelude::{
    ConnectionId, ControlEvent, MountId, ValidatedRegistration, WebviewCommand, WebviewEvent,
};
use orzma_webview_host::protocol::{RegisterKind, ServerMsg};
use serde_json::Value;
use std::collections::VecDeque;

/// A root pane whose connected program registered an inline page and
/// mounted it.
struct MountedPage {
    h: Harness,
    pane: PaneId,
    fake: FakePane,
    lines: Receiver<String>,
    instance: InstanceId,
    mount: MountId,
}

impl MountedPage {
    fn open() -> Self {
        let (mut h, control) = Harness::with_control();
        let (pane, fake) = h.open_root();
        let lines = connect(&mut h, &control, 1);
        let instance = register_inline(&mut h, &control, 1);
        h.drain();
        print_mounts(&fake, &[instance], 2);
        h.pump_pane(pane);
        let mount = webview_events(&h.drain())
            .into_iter()
            .find_map(|event| match event {
                WebviewEvent::Mounted { mount, .. } => Some(mount),
                _ => None,
            })
            .expect("the mount reaches the GUI");
        Self {
            h,
            pane,
            fake,
            lines,
            instance,
            mount,
        }
    }
}

/// Connects `connection` with the latest pane's token and returns the lines
/// the host pushes to it.
fn connect(h: &mut Harness, control: &Sender<ControlEvent>, connection: u64) -> Receiver<String> {
    let token = h
        .last_spawn_env_var("ORZMA_TOKEN")
        .expect("the pane got a token");
    let (writer, lines) = unbounded();
    let (reply, answer) = bounded(1);
    control
        .send(ControlEvent::Hello {
            connection: ConnectionId::new(connection),
            token,
            writer,
            reply,
        })
        .unwrap();
    h.drain_control();
    assert!(answer.try_recv().expect("an answer"), "the token resolves");
    lines
}

/// Registers an inline page for `connection` and returns its first instance.
fn register_inline(h: &mut Harness, control: &Sender<ControlEvent>, connection: u64) -> InstanceId {
    let (reply, answer) = bounded(1);
    control
        .send(ControlEvent::Register {
            connection: ConnectionId::new(connection),
            registration: ValidatedRegistration::try_from(RegisterKind::Inline {
                html: "<h1>x</h1>".into(),
                interactive: true,
                forward_keys: vec![],
                preload: vec![],
            }),
            reply,
        })
        .unwrap();
    h.drain_control();
    match answer.try_recv().expect("a reply") {
        ServerMsg::Registered { instance, .. } => instance.parse().expect("a wire instance"),
        other => panic!("expected Registered, got {other:?}"),
    }
}

/// Writes one APC mount per instance, each `rows` rows by 8 columns, into
/// `pane`'s output as a single chunk.
fn print_mounts(pane: &FakePane, instances: &[InstanceId], rows: u16) {
    let apc: String = instances
        .iter()
        .map(|instance| format!("\x1b_Omount;n={instance},r={rows},c=8\x1b\\"))
        .collect();
    pane.print(apc.as_bytes());
}

/// Splits `pane` vertically and returns the pane the split opened.
fn split(h: &mut Harness, pane: PaneId) -> PaneId {
    h.send(OrzmuxCommand::NewPane {
        request: RequestId(2),
        at: NewPaneAt::Split {
            pane: PaneTarget::Id(pane),
            orientation: SplitOrientation::Vertical,
        },
        cwd: None,
        env: vec![],
    });
    h.drain()
        .into_iter()
        .find_map(|event| match event {
            OrzmuxEvent::PaneOpened { pane, .. } => Some(pane),
            _ => None,
        })
        .expect("the split opens a pane")
}

fn webview_events(events: &VecDeque<OrzmuxEvent>) -> Vec<WebviewEvent<PaneId>> {
    events
        .iter()
        .filter_map(|event| match event {
            OrzmuxEvent::Webview { event, .. } => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// Asserts that a pane's shell starts with `ORZMA_SOCK` and a token when the
/// host has a socket, and with neither when it has none.
///
/// Case: orzma opens a pane with the control socket bound, and another
/// orzma opens one after the socket failed to bind.
#[test]
fn a_pane_gets_the_control_variables_only_with_a_socket() {
    let (mut with_socket, _control) = Harness::with_control();
    let _ = with_socket.open_root();
    assert_eq!(
        with_socket.last_spawn_env_var("ORZMA_SOCK").as_deref(),
        Some(CONTROL_SOCK)
    );
    assert!(
        with_socket
            .last_spawn_env_var("ORZMA_TOKEN")
            .is_some_and(|token| !token.is_empty())
    );
    let mut without = Harness::new();
    let _ = without.open_root();
    assert_eq!(without.last_spawn_env_var("ORZMA_SOCK"), None);
    assert_eq!(without.last_spawn_env_var("ORZMA_TOKEN"), None);
}

/// Asserts that an APC mount written by a registered program reaches the
/// GUI as a webview `Mounted` for that pane, and not as a raw VT signal.
///
/// Case: a markdown viewer registers its page and writes its mount.
#[test]
fn a_program_mount_reaches_the_gui_as_a_webview_mount() {
    let (mut h, control) = Harness::with_control();
    let (pane, fake) = h.open_root();
    let _lines = connect(&mut h, &control, 1);
    let instance = register_inline(&mut h, &control, 1);
    h.drain();
    print_mounts(&fake, &[instance], 2);
    h.pump_pane(pane);
    let events = h.drain();
    assert!(webview_events(&events).iter().any(|event| matches!(
        event,
        WebviewEvent::Mounted { pane: p, instance: i, .. } if *p == pane && *i == instance
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, OrzmuxEvent::Signal { .. }))
    );
}

/// Asserts that a second mount of a mounted instance reaches the GUI as a
/// resize of the same mount rather than as a new mount.
///
/// Case: a markdown viewer re-issues its mount with a taller rect after the
/// user enlarges the window.
#[test]
fn a_mount_of_a_mounted_instance_reaches_the_gui_as_a_resize() {
    let mut page = MountedPage::open();
    print_mounts(&page.fake, &[page.instance], 3);
    page.h.pump_pane(page.pane);
    assert_eq!(
        webview_events(&page.h.drain()),
        vec![WebviewEvent::Resized {
            mount: page.mount,
            size: PlacementSize { rows: 3, cols: 8 },
        }]
    );
}

/// Asserts that an APC mount of an instance nobody registered produces no
/// webview event.
///
/// Case: a program echoes a stale instance id from a previous session.
#[test]
fn an_unknown_mount_produces_no_webview_event() {
    let (mut h, _control) = Harness::with_control();
    let (pane, fake) = h.open_root();
    print_mounts(&fake, &[InstanceId(0xdead_beef)], 2);
    h.pump_pane(pane);
    assert!(webview_events(&h.drain()).is_empty());
}

/// Asserts that mounts the host refuses hand their VT reservations back, so
/// they cannot use up the pane's placement cap.
///
/// Case: a program writes mounts for ids it made up and for a page another
/// pane's program registered, trying to fill its terminal's placement table.
#[test]
fn refused_mounts_hand_their_vt_reservations_back() {
    let (mut h, control) = Harness::with_control();
    let (left, _left_fake) = h.open_root();
    let _left_lines = connect(&mut h, &control, 1);
    let foreign = register_inline(&mut h, &control, 1);
    let right = split(&mut h, left);
    let right_fake = h.spawned_pane().expect("the split spawned a pane");
    let _right_lines = connect(&mut h, &control, 2);
    let own = register_inline(&mut h, &control, 2);
    h.drain();
    let mut refused: Vec<InstanceId> = (1..MAX_PLACEMENTS as u128)
        .map(|n| InstanceId(0xbad0 + n))
        .collect();
    refused.push(foreign);
    print_mounts(&right_fake, &refused, 2);
    h.pump_pane(right);
    assert!(webview_events(&h.drain()).is_empty());
    print_mounts(&right_fake, &[own], 2);
    h.pump_pane(right);
    assert!(webview_events(&h.drain()).iter().any(|event| matches!(
        event,
        WebviewEvent::Mounted { pane, instance, .. } if *pane == right && *instance == own
    )));
}

/// Asserts that closing a pane reports its page's unmount and asset release
/// before `PaneClosed`.
///
/// Case: the user kills a pane whose program shows a page.
#[test]
fn closing_a_pane_reports_its_webview_teardown_first() {
    let mut page = MountedPage::open();
    page.h.send(OrzmuxCommand::KillPane {
        pane: PaneTarget::Id(page.pane),
    });
    let events: Vec<OrzmuxEvent> = page.h.drain().into_iter().collect();
    let unmounted = events
        .iter()
        .position(|event| {
            matches!(
                event,
                OrzmuxEvent::Webview { event: WebviewEvent::Unmounted { mounts }, .. }
                    if mounts == &vec![page.mount]
            )
        })
        .expect("the unmount is reported");
    let released = events
        .iter()
        .position(|event| {
            matches!(
                event,
                OrzmuxEvent::Webview {
                    event: WebviewEvent::AssetReleased { .. },
                    ..
                }
            )
        })
        .expect("the asset release is reported");
    let closed = events
        .iter()
        .position(|event| matches!(event, OrzmuxEvent::PaneClosed { .. }))
        .expect("the pane closes");
    assert!(unmounted < closed && released < closed);
}

/// Asserts that a socket mount reaches the pane's VT and comes back as a
/// webview mount.
///
/// Case: orzmd in a Windows pane mounts its page over the socket because
/// ConPTY dropped its APC.
#[test]
fn a_socket_mount_comes_back_as_a_webview_mount() {
    let (mut h, control) = Harness::with_control();
    let (pane, _fake) = h.open_root();
    let _lines = connect(&mut h, &control, 1);
    let instance = register_inline(&mut h, &control, 1);
    h.drain();
    control
        .send(ControlEvent::Mount {
            connection: ConnectionId::new(1),
            instance: instance.to_string(),
            row: 1,
            col: 2,
            rows: 4,
            cols: 8,
        })
        .unwrap();
    h.drain_control();
    h.pump_pane(pane);
    assert!(webview_events(&h.drain()).iter().any(|event| matches!(
        event,
        WebviewEvent::Mounted { instance: i, .. } if *i == instance
    )));
}

/// Asserts that a GUI focus is applied and answered with the sequence of the
/// command that asked for it.
///
/// Case: the user clicks a page and the GUI reconciles its optimistic focus
/// against the answer.
#[test]
fn a_webview_command_is_answered_with_its_seq() {
    let mut page = MountedPage::open();
    let seq = page.h.send(OrzmuxCommand::Webview(WebviewCommand::Focus {
        mount: Some(page.mount),
    }));
    assert!(page.h.drain().iter().any(|event| matches!(
        event,
        OrzmuxEvent::Webview { event: WebviewEvent::FocusChanged { focused: Some(m) }, seq: s }
            if *m == page.mount && *s == seq
    )));
}

/// Asserts that selecting another pane releases the focus a page holds in
/// the first one, and tells the page's program.
///
/// Case: the user splits the window, focuses a page in the left pane, and
/// then selects the right pane.
#[test]
fn selecting_another_pane_releases_a_focused_page() {
    let mut page = MountedPage::open();
    let right = split(&mut page.h, page.pane);
    page.h.send(OrzmuxCommand::Webview(WebviewCommand::Focus {
        mount: Some(page.mount),
    }));
    let _ = page.h.drain();
    let _ = page.lines.try_iter().count();
    page.h.send(OrzmuxCommand::SelectPane { pane: right });
    assert!(
        webview_events(&page.h.drain()).contains(&WebviewEvent::FocusChanged { focused: None })
    );
    let pushed: Vec<Value> = page
        .lines
        .try_iter()
        .map(|line| serde_json::from_str(&line).unwrap())
        .collect();
    assert!(
        pushed
            .iter()
            .any(|push| push["op"] == "focus_changed" && push["focused"] == false)
    );
}

/// Asserts that a webview command the host refuses changes nothing and does
/// not stop the loop.
///
/// Case: the GUI reports the first frame of a page whose mount the host has
/// already ended.
#[test]
fn a_refused_webview_command_changes_nothing() {
    let mut page = MountedPage::open();
    page.h
        .send(OrzmuxCommand::Webview(WebviewCommand::Composited {
            mount: MountId::new(9_999),
        }));
    assert!(webview_events(&page.h.drain()).is_empty());
    let seq = page.h.send(OrzmuxCommand::Webview(WebviewCommand::Focus {
        mount: Some(page.mount),
    }));
    assert!(page.h.drain().iter().any(|event| matches!(
        event,
        OrzmuxEvent::Webview { event: WebviewEvent::FocusChanged { focused: Some(m) }, seq: s }
            if *m == page.mount && *s == seq
    )));
}
