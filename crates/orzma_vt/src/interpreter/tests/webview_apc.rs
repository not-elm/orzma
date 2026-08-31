//! Tests for the application program command that mounts and unmounts
//! a webview placement.

use super::*;

/// Asserts that an application program command is ignored rather
/// than fatal, and that the parser returns to ground behind it.
///
/// Case: a program probes for the webview verbs on a terminal whose
/// APC handler has not landed yet.
#[test]
fn an_application_program_command_is_ignored_rather_than_fatal() {
    let device = interpret(b"\x1b_hi\x1b\\a");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
}

/// Asserts that an APC mount mints a placement and reports it
/// alongside the verb it parsed.
///
/// Case: a companion app registers a view over the control socket
/// and writes the mount sequence to reserve room for its page.
#[test]
fn an_apc_mount_reports_the_placement_it_minted() {
    let (_device, output) = interpret_fully(b"\x1b_Omount;v=memo,r=2,c=3\x1b\\");
    assert_eq!(
        output.signals,
        vec![VtSignal::WebviewApc {
            verb: WebviewApcVerb::Mount {
                view_id: "memo".to_owned(),
                rows: 2,
                cols: 3,
                instance_id: None,
            },
            placement: Some(PlacementId(0)),
        }]
    );
}

/// Asserts that an APC-mounted placement is listed in the next
/// emitted frame.
///
/// Case: a companion app mounts its view, and the host must be told
/// where to draw the webview on the frame that follows.
#[test]
fn an_apc_mount_reaches_the_next_frame() {
    let mut session = Session::new();
    session.feed(b"\x1b_Omount;v=memo,r=2,c=3\x1b\\");
    let frame = session.frame().expect("a mount emits");
    let listed: Vec<PlacementId> = frame
        .placements
        .expect("a placement change is listed")
        .iter()
        .map(|placement| placement.id)
        .collect();
    assert_eq!(listed, vec![PlacementId(0)]);
}

/// Asserts that an APC unmount reports the verb with no placement
/// and drops the mounted view from the next frame.
///
/// Case: a companion app tears its view down on the way out.
#[test]
fn an_apc_unmount_reports_no_placement() {
    let mut session = Session::new();
    session.feed(b"\x1b_Omount;v=memo,r=2,c=3\x1b\\");
    session.frame();
    let output = session.feed(b"\x1b_Ounmount;v=memo\x1b\\");
    assert_eq!(
        output.signals,
        vec![VtSignal::WebviewApc {
            verb: WebviewApcVerb::Unmount {
                view_id: Some("memo".to_owned()),
                instance_id: None,
            },
            placement: None,
        }]
    );
    let frame = session.frame().expect("the unmount emits");
    assert_eq!(frame.placements, Some(vec![]));
}

/// Asserts that an APC payload which is not an orzma webview verb
/// raises no signal.
///
/// Case: a program on the same terminal writes a kitty graphics APC
/// sequence, which orzma must leave alone.
#[test]
fn a_foreign_apc_payload_raises_no_signal() {
    let (_device, output) = interpret_fully(b"\x1b_Ga=T,f=100\x1b\\");
    assert!(output.signals.is_empty());
}

/// Asserts that a mount past the per-terminal cap still reports the
/// verb, with no placement for the consumer to drop.
///
/// Case: a program mounts more views than the terminal has overlay
/// slots for.
#[test]
fn an_apc_mount_past_the_cap_reports_no_placement() {
    let mut session = Session::new();
    for i in 0..MAX_PLACEMENTS {
        session.mount(&format!("v{i}"));
    }
    let output = session.feed(b"\x1b_Omount;v=over,r=1,c=1\x1b\\");
    assert_eq!(
        output.signals,
        vec![VtSignal::WebviewApc {
            verb: WebviewApcVerb::Mount {
                view_id: "over".to_owned(),
                rows: 1,
                cols: 1,
                instance_id: None,
            },
            placement: None,
        }]
    );
}
