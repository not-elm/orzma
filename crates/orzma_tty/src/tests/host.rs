//! Tests for the host webview mount and removal signals.

use super::*;

/// Asserts that a host-driven removal arms the coalescer only when a
/// placement actually went.
///
/// Case: two connections drop in the same tick and the host issues a
/// removal for each, but only the first names a live placement.
#[test]
fn a_host_removal_arms_the_coalescer_only_when_something_went() {
    let id: InstanceId = "3f5a9c02d1e84b7690ab3cde12f45678"
        .parse()
        .expect("valid id");
    let mut tty = OrzmaTty::detached(
        OrzmaVt::new(grid(80, 24), 100),
        grid(80, 24),
        Box::new(CaptureSink::default()),
    )
    .expect("the detached constructor succeeds");
    tty.feed_bytes(format!("\x1b_Omount;n={id},r=4,c=8\x1b\\").as_bytes());
    let _ = tty.pump();

    // NOTE: disarm explicitly between the two probes. A second pump()
    // would not disarm on its own — the bootstrap debt is already spent
    // and the 3 ms IDLE window has not elapsed — so the assertion below
    // would read the arming left by the first removal.
    tty.coalescer.disarm();
    tty.remove_placements(&[id]);
    assert!(
        tty.coalescer.is_armed(),
        "a real removal arms the coalescer"
    );

    tty.coalescer.disarm();
    tty.remove_placements(&[id]);
    assert!(
        !tty.coalescer.is_armed(),
        "a removal that names nothing does not"
    );
}

/// Asserts that an accepted host-driven mount queues `WebviewMount`
/// for the next pump and arms the coalescer.
///
/// Case: the control plane relays a socket `mount` from orzmd running
/// in a Windows pane.
#[test]
fn a_host_mount_queues_the_mount_signal_and_arms_the_coalescer() {
    let (mut tty, _sink) = detached_term();
    let size = PlacementSize { rows: 4, cols: 8 };
    tty.coalescer.disarm();

    tty.mount_placement_at(InstanceId(7), ScreenLine(1), GridColumn(2), size);

    assert!(
        tty.coalescer.is_armed(),
        "an accepted mount arms the coalescer"
    );
    assert_eq!(
        tty.vt.mounts,
        vec![(ScreenLine(1), GridColumn(2), size, InstanceId(7))]
    );
    let out = tty.flush_now();
    assert_eq!(
        out.signals().cloned().collect::<Vec<_>>(),
        vec![TtySignal::Vt(VtSignal::WebviewMount {
            instance: InstanceId(7),
            size
        })]
    );
}

/// Asserts that a rejected host-driven mount queues
/// `WebviewMountRejected` and leaves the coalescer alone.
///
/// Case: the socket `mount` names a row the pane no longer has after a
/// resize, so the VT refuses it.
#[test]
fn a_rejected_host_mount_queues_the_rejection_without_arming() {
    let (mut tty, _sink) = detached_term();
    tty.vt.mount_accepts = false;
    tty.coalescer.disarm();

    tty.mount_placement_at(
        InstanceId(7),
        ScreenLine(99),
        GridColumn(2),
        PlacementSize { rows: 4, cols: 8 },
    );

    assert!(!tty.coalescer.is_armed(), "a rejected mount arms nothing");
    let out = tty.flush_now();
    assert_eq!(
        out.signals().cloned().collect::<Vec<_>>(),
        vec![TtySignal::Vt(VtSignal::WebviewMountRejected {
            instance: InstanceId(7)
        })]
    );
}
