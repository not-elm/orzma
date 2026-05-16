//! Per-Activity bridge task. Pure frame listener: subscribes to
//! `Page.screencastFrame`, immediately acks each one, decodes the JPEG bytes,
//! and publishes `BrowserSnapshot`s to a shared `watch::Sender`.
//!
//! Acks every CDP frame immediately on receipt so Chromium's memory never
//! grows with unacked frames (puppeteer#11062 confirms immediate ack bounds
//! memory but does not bound Chromium CPU — CPU is bounded by `every_nth_frame`,
//! `quality`, `max_width`, and by stopping the screencast for inactive
//! activities; the active-Activity pause/resume is wired by Task 2.8/3.5).
//!
//! Bridge ↔ PageActor split:
//! - PageActor owns ALL `Page.startScreencast` / `Page.stopScreencast` calls
//!   (initial start, resize restart, navigation restart, pause, resume).
//! - Bridge is a pure listener: registers the `EventScreencastFrame` handler
//!   and acks + publishes each frame. It never starts or stops the screencast.
//! - This ensures the very first frame is sized to the actual pane (not to a
//!   hardcoded 1920×1200 default), fixing the initial-delay bug.

use crate::snapshot::{BrowserSnapshot, NavState, ScreencastFrame};
use base64::Engine as _;
use chromiumoxide::cdp::browser_protocol::page as cdp_page;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Default JPEG quality for screencasting (0-100).
pub(crate) const DEFAULT_JPEG_QUALITY: i64 = 55;
/// Default frame-skip factor: emit every Nth frame.
pub(crate) const DEFAULT_EVERY_NTH_FRAME: i64 = 1;

/// Build a `StartScreencastParams` with project-wide defaults for format,
/// quality, and frame rate, and caller-supplied pixel bounds.
pub(crate) fn start_screencast_params(
    max_width: i64,
    max_height: i64,
) -> cdp_page::StartScreencastParams {
    cdp_page::StartScreencastParams::builder()
        .format(cdp_page::StartScreencastFormat::Jpeg)
        .quality(DEFAULT_JPEG_QUALITY)
        .max_width(max_width)
        .max_height(max_height)
        .every_nth_frame(DEFAULT_EVERY_NTH_FRAME)
        .build()
}

/// Run the bridge task. Subscribes to `Page.screencastFrame`, immediately
/// acks each one, decodes the JPEG bytes, and publishes a new
/// `Arc<BrowserSnapshot>` through `sender`. Returns when `cancel` fires or
/// the stream closes.
///
/// # NOTE
/// The bridge never calls `Page.startScreencast` or `Page.stopScreencast`.
/// `PageActor` owns that lifecycle so the very first frame is sized to the
/// actual pane viewport, not to a hardcoded default.
pub(crate) async fn run(
    page: chromiumoxide::Page,
    sender: watch::Sender<Arc<BrowserSnapshot>>,
    cancel: CancellationToken,
) {
    let mut frames = match page
        .event_listener::<cdp_page::EventScreencastFrame>()
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "screencastFrame listener failed");
            return;
        }
    };

    let mut load_events = page
        .event_listener::<cdp_page::EventLoadEventFired>()
        .await
        .ok();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            evt = frames.next() => {
                let Some(frame) = evt else { break };
                // NOTE: ack immediately — independent of any subscriber on `sender`.
                let _ = page.execute(cdp_page::ScreencastFrameAckParams { session_id: frame.session_id }).await;
                let jpeg = decode_frame_data(&frame);
                let (width, height) = frame_dimensions(&frame);
                let mut snap = sender.borrow().as_ref().clone();
                snap.frame = Some(ScreencastFrame { jpeg: bytes::Bytes::from(jpeg), width, height });
                let _ = sender.send(Arc::new(snap));
            }
            _ = async {
                if let Some(s) = load_events.as_mut() {
                    let _ = s.next().await;
                } else {
                    futures_util::future::pending::<()>().await
                }
            } => {
                if let Some(nav) = refresh_nav(&page).await {
                    let mut snap = sender.borrow().as_ref().clone();
                    snap.nav = nav;
                    let _ = sender.send(Arc::new(snap));
                }
            }
        }
    }
}

/// Decode a `EventScreencastFrame.data` into raw JPEG bytes.
///
/// In chromiumoxide 0.7.0, `EventScreencastFrame.data` is
/// `chromiumoxide_types::Binary`, a newtype over `String` whose inner value
/// is a base64-encoded JPEG as emitted by Chromium's CDP protocol. We decode
/// it using the standard base64 alphabet; malformed frames produce an empty
/// `Vec<u8>` so the stream keeps running.
fn decode_frame_data(frame: &cdp_page::EventScreencastFrame) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(frame.data.as_ref() as &str)
        .unwrap_or_default()
}

/// Read width/height from the frame metadata.
///
/// `ScreencastFrameMetadata.device_width` and `device_height` are `f64`
/// (device pixels in DIP per the CDP spec). We truncate to `u32`; a value
/// of zero is returned on overflow, letting the downstream renderer cope.
fn frame_dimensions(frame: &cdp_page::EventScreencastFrame) -> (u32, u32) {
    let w = frame.metadata.device_width as u32;
    let h = frame.metadata.device_height as u32;
    (w, h)
}

/// Query the live page for its current URL and title, returning a fresh
/// `NavState`. Returns `None` if any query fails (best-effort).
async fn refresh_nav(page: &chromiumoxide::Page) -> Option<NavState> {
    let url = page.url().await.ok().flatten().unwrap_or_default();
    let title = page.get_title().await.ok().flatten().unwrap_or_default();
    Some(NavState { url, title })
}
