//! Integration test (Plan 2 Task A14): spawn the real cef_host binary,
//! complete the daemon-side handshake (Hello/Ready), then request a
//! BrowserCreate(about:blank) carrying its own shm fd via SCM_RIGHTS,
//! wait for `BrowserReady`, and poll the shm ring for the first BGRA
//! keyframe.
//!
//! Gated by `OZMUX_TEST_REAL_CEF=1` because it requires a built cef_host
//! binary and a working CEF framework on disk; CI does not run it.

use ozmux_browser::cef_service::CefHostSupervisor;
use ozmux_browser::shm_alloc::{self, SLOT_PAYLOAD_MAX};
use ozmux_browser::shm_reader::ShmReader;
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{BrowserProfileWire, HostEvent};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn handshake_then_one_frame() {
    if std::env::var("OZMUX_TEST_REAL_CEF").ok().as_deref() != Some("1") {
        eprintln!("skipped; set OZMUX_TEST_REAL_CEF=1");
        return;
    }

    let socket = std::path::PathBuf::from("/tmp/ozmux_test_handshake.sock");
    let _ = std::fs::remove_file(&socket);

    let supervisor = CefHostSupervisor::new(socket);
    let handles =
        tokio::time::timeout(Duration::from_secs(15), supervisor.spawn_and_handshake())
            .await
            .expect("handshake timed out")
            .expect("handshake failed");

    let aid = ActivityId(format!("test-{}", uuid::Uuid::new_v4()));
    let shm_fd = shm_alloc::create_shm_for_activity(&aid.0, SLOT_PAYLOAD_MAX)
        .expect("shm_alloc::create_shm_for_activity");
    // dup the fd so we can mmap a reader-side view while sending the original
    // to cef_host via SCM_RIGHTS. The reader copy is dropped at test end.
    let shm_for_read = shm_fd.try_clone().expect("OwnedFd::try_clone");

    let len = ShmReader::required_region_size(SLOT_PAYLOAD_MAX);
    // SAFETY: shm_for_read is a valid mmap-able fd. We map READ-only because
    // the cef_host writer side will be the only mutator. The pointer is held
    // for the duration of the test; we leak the munmap on exit since the
    // test process tears down anyway.
    let base = unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            shm_for_read.as_raw_fd(),
            0,
        );
        assert!(p != libc::MAP_FAILED, "mmap failed");
        p as *const u8
    };
    // SAFETY: `base` is a valid mmap region of `len` bytes laid out per
    // shm_writer / shm_reader's shared layout (same SLOT_PAYLOAD_MAX on
    // both sides).
    let reader = unsafe { ShmReader::from_mmap(base, SLOT_PAYLOAD_MAX) };

    handles
        .request_browser_create(
            aid.clone(),
            "about:blank".into(),
            1,
            Vec::new(),
            BrowserProfileWire::Named { name: "default".into() },
            shm_fd,
        )
        .await
        .expect("request_browser_create");

    // Wait for BrowserReady matching our aid.
    // NOTE: events is now Option-wrapped inside a Mutex so spawn_event_pump can
    // take it at daemon startup without needing &mut CefHostHandles. Tests that
    // need raw access take the receiver here before any pump is spawned.
    let mut events_rx = handles
        .events_take()
        .expect("events receiver not available");
    let ready_deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if Instant::now() > ready_deadline {
            panic!("BrowserReady never arrived in 20s");
        }
        match tokio::time::timeout(Duration::from_millis(500), events_rx.recv()).await {
            Ok(Some(HostEvent::BrowserReady {
                aid: ev_aid,
                ok_or_err,
            })) if ev_aid == aid => {
                ok_or_err.expect("BrowserReady reported an error");
                break;
            }
            Ok(Some(_)) | Ok(None) | Err(_) => continue,
        }
    }

    // Poll shm for the first keyframe (1280×800 BGRA).
    let frame_deadline = Instant::now() + Duration::from_secs(30);
    let mut frame_observed = false;
    while Instant::now() < frame_deadline {
        let lap = reader.current_lap();
        if lap == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }
        let slot_idx = ((lap - 1) as usize) % 4;
        if let Some(snap) = reader.read_stable(slot_idx) {
            assert_eq!(snap.width, 1280, "unexpected frame width");
            assert_eq!(snap.height, 800, "unexpected frame height");
            assert!(snap.is_keyframe, "first observed frame must be a keyframe");
            assert_eq!(
                snap.payload.len(),
                1280 * 800 * 4,
                "keyframe BGRA payload must be width * height * 4 bytes"
            );
            frame_observed = true;
            break;
        }
    }
    assert!(frame_observed, "no keyframe observed in shm within 30s");

    if let Some(mut child) = handles.take_child() {
        let _ = child.kill().await;
    }
}
