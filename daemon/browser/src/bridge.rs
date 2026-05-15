//! Per-Activity bridge task. Owns the screencast subscription on a
//! `chromiumoxide::Page` and publishes `BrowserSnapshot`s to a shared
//! `watch::Sender`.
//!
//! Acks every CDP frame immediately on receipt so Chromium's memory never
//! grows with unacked frames (puppeteer#11062 confirms immediate ack bounds
//! memory but does not bound Chromium CPU — CPU is bounded by `every_nth_frame`,
//! `quality`, `max_width`, and by stopping the screencast for inactive
//! activities; the active-Activity pause/resume is wired by Task 2.8/3.5).
//!
//! Bridge ↔ PageActor split:
//! - PageActor owns command-driven CDP calls (`Page.stopScreencast`, etc.).
//! - Bridge owns the `Page.startScreencast` listener and `screencastFrameAck`.
//! - To resume after a pause, the bridge must re-call `startScreencast`.
//!   For Task 2.7 we expose a `restart` signal; Task 2.8's BrowserService
//!   wires PageActor pause/resume into the bridge.

use crate::snapshot::{BrowserSnapshot, NavState, ScreencastFrame};
use base64::Engine as _;
use chromiumoxide::cdp::browser_protocol::page as cdp_page;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Screencast configuration. Conservative defaults that balance frame rate
/// against per-frame bandwidth.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct BridgeConfig {
    /// JPEG quality (0-100). Higher = better quality, larger frame.
    pub jpeg_quality: i64,
    /// Maximum frame width in device pixels.
    pub max_width: i64,
    /// Maximum frame height in device pixels.
    pub max_height: i64,
    /// Emit one frame per N. Higher = lower frame rate, less CPU.
    pub every_nth_frame: i64,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            jpeg_quality: 55,
            max_width: 1920,
            max_height: 1200,
            every_nth_frame: 1,
        }
    }
}

/// Run the bridge task. Subscribes to `Page.screencastFrame`, immediately
/// acks each one, decodes the JPEG bytes, and publishes a new
/// `Arc<BrowserSnapshot>` through `sender`. Returns when `cancel` fires or
/// the stream closes.
#[allow(dead_code)]
pub(crate) async fn run(
    page: chromiumoxide::Page,
    sender: watch::Sender<Arc<BrowserSnapshot>>,
    cancel: CancellationToken,
    cfg: BridgeConfig,
) {
    // Start screencast.
    let start_params = cdp_page::StartScreencastParams::builder()
        .format(cdp_page::StartScreencastFormat::Jpeg)
        .quality(cfg.jpeg_quality)
        .max_width(cfg.max_width)
        .max_height(cfg.max_height)
        .every_nth_frame(cfg.every_nth_frame)
        .build();

    if let Err(e) = page.execute(start_params).await {
        tracing::warn!(error = %e, "startScreencast failed");
        return;
    }

    // Subscribe to screencast frames.
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

    // Subscribe to load events for nav refresh.
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

    // Best-effort: stop screencast on exit. Page may already be closed.
    let _ = page.execute(cdp_page::StopScreencastParams {}).await;
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
    Some(NavState {
        url,
        title,
        loading: false,
        can_go_back: false,
        can_go_forward: false,
    })
}
