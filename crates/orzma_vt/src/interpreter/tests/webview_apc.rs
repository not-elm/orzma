//! Tests for the application program command that mounts and unmounts
//! a webview placement.

use super::*;

const ID: &str = "3f5a9c02d1e84b7690ab3cde12f45678";

/// Asserts that an application program command is ignored rather
/// than fatal, and that the parser returns to ground behind it.
///
/// Case: a program on this terminal writes an APC sequence that is not
/// an orzma webview verb.
#[test]
fn an_application_program_command_is_ignored_rather_than_fatal() {
    let device = interpret(b"\x1b_hi\x1b\\a");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
}

/// Asserts that an APC mount registers a placement and reports it with
/// the reservation the payload asked for.
///
/// Case: a companion app registers an instance over the control socket
/// and writes the mount sequence to reserve room for its page.
#[test]
fn an_apc_mount_reports_the_placement_it_registered() {
    let id: InstanceId = ID.parse().expect("the fixture is a valid id");
    let (_device, output) = interpret_fully(format!("\x1b_Omount;n={id},r=2,c=3\x1b\\").as_bytes());
    assert_eq!(
        output.signals,
        vec![VtSignal::WebviewMount {
            instance: id,
            size: PlacementSize { rows: 2, cols: 3 },
        }]
    );
}

/// Asserts that an APC-mounted placement is listed in the next
/// emitted frame.
///
/// Case: a companion app mounts its instance, and the host must be told
/// where to draw the webview on the frame that follows.
#[test]
fn an_apc_mount_reaches_the_next_frame() {
    let id: InstanceId = ID.parse().expect("the fixture is a valid id");
    let mut session = Session::new();
    session.feed(format!("\x1b_Omount;n={id},r=2,c=3\x1b\\").as_bytes());
    let placements = session.listed_placements();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].id, id);
}

/// Asserts that an APC unmount reports the instance it named and drops
/// the mounted placement from the next frame.
///
/// Case: a companion app tears its instance down on the way out.
#[test]
fn an_apc_unmount_reports_the_address_it_named() {
    let id: InstanceId = ID.parse().expect("the fixture is a valid id");
    let mut session = Session::new();
    session.feed(format!("\x1b_Omount;n={id},r=2,c=3\x1b\\").as_bytes());
    session.frame();
    let output = session.feed(format!("\x1b_Ounmount;n={id}\x1b\\").as_bytes());
    assert_eq!(
        output.signals,
        vec![VtSignal::WebviewUnmount { instance: Some(id) }]
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

/// Asserts that a mount past the per-terminal cap is reported as a
/// rejection naming the instance it refused, rather than as a mount.
///
/// Case: a program mounts more instances than the terminal has overlay
/// slots for.
#[test]
fn an_apc_mount_past_the_cap_is_rejected() {
    let mut session = Session::new();
    for i in 0..MAX_PLACEMENTS as u128 {
        session.mount(InstanceId(i));
    }
    let over: InstanceId = ID.parse().expect("the fixture is a valid id");
    let output = session.feed(format!("\x1b_Omount;n={over},r=1,c=1\x1b\\").as_bytes());
    assert_eq!(
        output.signals,
        vec![VtSignal::WebviewMountRejected { instance: over }]
    );
}

/// Asserts that an accepted mount raises the chunk liveness.
///
/// Case: a companion app mounts its instance in the very first chunk
/// the terminal ever interprets, with no other output alongside it.
#[test]
fn an_accepted_mount_raises_the_chunk_liveness() {
    assert!(damage_of(
        format!("\x1b_Omount;n={ID},r=2,c=3\x1b\\").as_bytes()
    ));
}

/// Asserts that a hit unmount raises the chunk liveness.
///
/// Case: a companion app tears its instance down in a chunk that
/// carries no other terminal output.
#[test]
fn a_hit_unmount_raises_the_chunk_liveness() {
    assert!(liveness_after(
        format!("\x1b_Omount;n={ID},r=2,c=3\x1b\\").as_bytes(),
        format!("\x1b_Ounmount;n={ID}\x1b\\").as_bytes()
    ));
}

/// Asserts that a mount the cap rejected does not raise the chunk
/// liveness.
///
/// Case: a program mounts past the per-terminal overlay-slot cap in a
/// chunk that carries no other terminal output.
#[test]
fn a_capped_mount_does_not_raise_the_chunk_liveness() {
    let mut session = Session::new();
    for i in 0..MAX_PLACEMENTS as u128 {
        session.mount(InstanceId(i));
    }
    let output = session.feed(format!("\x1b_Omount;n={ID},r=1,c=1\x1b\\").as_bytes());
    assert!(!output.damaged);
}

/// Asserts that a re-mount of a live instance keeps the id and reports
/// the new reservation, rather than minting a successor id.
///
/// Case: a program redraws the same view one row shorter after the
/// surrounding layout changed.
#[test]
fn a_remount_of_a_live_instance_keeps_its_id() {
    let id: InstanceId = ID.parse().expect("the fixture is a valid id");
    let mut vt = OrzmaVt::new(GridSize { cols: 80, rows: 24 }, 100);
    vt.interpret(format!("\x1b_Omount;n={id},r=10,c=40\x1b\\").as_bytes());
    let out = vt.interpret(format!("\x1b_Omount;n={id},r=9,c=40\x1b\\").as_bytes());
    assert_eq!(
        out.signals,
        vec![VtSignal::WebviewMount {
            instance: id,
            size: PlacementSize { rows: 9, cols: 40 },
        }]
    );
    let frame = vt.frame().expect("the remount damages the chunk");
    let placements = frame.placements.expect("the list changed");
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].id, id);
    assert_eq!(placements[0].size, PlacementSize { rows: 9, cols: 40 });
}

/// Line feeds that carry row 0 of the harness's 3-row, 10-row-history
/// session out of the ring: two reach the bottom row, ten fill the
/// history, and one more discards the oldest row.
const LINE_FEEDS_PAST_THE_CAP: usize = 13;

/// Asserts that output pushing a placement's anchor row past the
/// history cap names the placement in that chunk's signals and marks
/// the chunk damaged.
///
/// Case: a companion app mounted a webview beside a prompt, and a
/// long build then prints more lines than the scrollback keeps.
#[test]
fn output_past_the_history_cap_evicts_the_placement_in_its_own_chunk() {
    let mut session = Session::new();
    let id = InstanceId(1);
    session.mount(id);
    let output = session.feed(&b"\n".repeat(LINE_FEEDS_PAST_THE_CAP));
    assert_eq!(
        output.signals,
        vec![VtSignal::WebviewEvicted {
            placements: vec![id]
        }]
    );
    assert!(output.damaged);
}

/// Asserts that output which only scrolls a placement's anchor row
/// into history, below the cap, evicts nothing and lists the placement
/// at the history line its anchor scrolled to.
///
/// Case: a companion app mounted a webview beside a prompt and the
/// shell printed a few more lines under it.
#[test]
fn output_that_keeps_the_anchor_in_history_evicts_nothing() {
    let mut session = Session::new();
    let id = InstanceId(1);
    session.mount(id);
    session.frame();
    let output = session.feed(&b"\n".repeat(5));
    assert!(output.signals.is_empty());
    let placements = session.listed_placements();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].id, id);
    assert_eq!(placements[0].point.line, GridLine(-3));
}
