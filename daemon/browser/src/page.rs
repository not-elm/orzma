//! Per-page actor. Owns a `chromiumoxide::Page` and serializes CDP calls so
//! the registry lock in `BrowserService` never holds across an `await`. One
//! actor per browser Activity.

use crate::bridge::start_screencast_params;
use crate::error::{BrowserError, BrowserResult};
use crate::input::{
    ime_commit_to_cdp, ime_composition_to_cdp, key_to_cdp, mouse_to_cdp, paste_to_cdp, wheel_to_cdp,
};
use crate::snapshot::{BrowserSnapshot, Viewport};
use crate::wire::{BrowserClientMsg, NavCommand};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch};

/// Command sent to a `PageActor` via its `mpsc::Sender`.
#[derive(Debug)]
pub(crate) enum PageCommand {
    /// Translate a wire input message and forward it to CDP.
    Input(BrowserClientMsg),
    /// Drive page navigation (Navigate/Back/Forward/Reload/Stop).
    Nav(NavCommand),
    /// Update the page's emulated viewport and restart the screencast at the
    /// DPR-adjusted pixel dimensions.
    Resize {
        /// Viewport width in CSS pixels.
        width: u32,
        /// Viewport height in CSS pixels.
        height: u32,
        /// `window.devicePixelRatio` from the frontend. Scales the JPEG
        /// screencast bounds for crisp HiDPI rendering.
        device_scale_factor: f64,
    },
    /// Pause screencast (CDP `Page.stopScreencast`).
    PauseScreencast,
    /// Resume screencast — re-issue `Page.startScreencast` using the last
    /// known viewport dimensions so the frame size is correct after a pause.
    ResumeScreencast,
    /// Reply with the page's current selection text via `Runtime.evaluate`.
    GetSelection(oneshot::Sender<String>),
    /// Notification from the bridge that Chromium's main frame finished loading.
    /// Used to re-issue `startScreencast` after in-page navigations, which
    /// don't go through the `Nav` command path but do swap renderers on
    /// cross-origin transitions.
    OnMainFrameLoaded,
    /// Stop the actor; close the page.
    Close,
}

/// Persistent state maintained across actor command iterations.
struct PageState {
    /// Latest (css_width, css_height, device_scale_factor) received from the
    /// frontend. `None` until the first `Resize` command arrives.
    viewport: Option<(u32, u32, f64)>,
}

impl PageState {
    /// Returns the `(bitmap_width, bitmap_height)` in physical pixels for the
    /// current viewport, or `None` if no viewport has been set yet.
    fn bitmap_dims(&self) -> Option<(i64, i64)> {
        self.viewport.map(|(w, h, dsf)| {
            (
                (w as f64 * dsf).ceil() as i64,
                (h as f64 * dsf).ceil() as i64,
            )
        })
    }
}

/// Run the actor loop. Consumes the `Page`, the `mpsc::Receiver` end of the
/// actor command channel, and the snapshot watch sender so `Resize` can
/// publish the updated `Viewport`. Returns when the channel closes or a
/// `Close` command arrives. The owning `BrowserService` task spawns this on a
/// dedicated tokio task per Activity.
pub(crate) async fn run(
    page: chromiumoxide::Page,
    mut rx: mpsc::Receiver<PageCommand>,
    snapshot_tx: watch::Sender<Arc<BrowserSnapshot>>,
) -> BrowserResult<()> {
    let mut state = PageState { viewport: None };
    while let Some(cmd) = rx.recv().await {
        if matches!(cmd, PageCommand::Close) {
            break;
        }
        if let Err(e) = handle(&page, &mut state, &snapshot_tx, cmd).await {
            tracing::warn!(error = %e, "page command failed");
        }
    }
    let _ = page.close().await;
    Ok(())
}

/// Stop then start the screencast so that the frame stream uses `bitmap_w` ×
/// `bitmap_h` physical pixels. Used by `Resize`, `ResumeScreencast`, and Nav
/// post-await to re-subscribe after a possible renderer swap.
///
/// # NOTE
/// `Page.startScreencast` is idempotent w.r.t. session-id reset (Chromium
/// `page_handler.cc`), but explicit stop+start is the documented pattern.
async fn restart_screencast(
    page: &chromiumoxide::Page,
    bitmap_w: i64,
    bitmap_h: i64,
) -> BrowserResult<()> {
    use chromiumoxide::cdp::browser_protocol::page as cdp_page;
    let _ = page
        .execute(cdp_page::StopScreencastParams::default())
        .await;
    page.execute(start_screencast_params(bitmap_w, bitmap_h))
        .await
        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
    Ok(())
}

async fn handle(
    page: &chromiumoxide::Page,
    state: &mut PageState,
    snapshot_tx: &watch::Sender<Arc<BrowserSnapshot>>,
    cmd: PageCommand,
) -> BrowserResult<()> {
    use chromiumoxide::cdp::browser_protocol::emulation as cdp_em;
    use chromiumoxide::cdp::browser_protocol::page as cdp_page;

    match cmd {
        PageCommand::Input(msg) => match &msg {
            BrowserClientMsg::Mouse { .. } => {
                if let Some(p) = mouse_to_cdp(&msg) {
                    page.execute(p)
                        .await
                        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
                }
            }
            BrowserClientMsg::Wheel { .. } => {
                if let Some(p) = wheel_to_cdp(&msg) {
                    page.execute(p)
                        .await
                        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
                }
            }
            BrowserClientMsg::Key { .. } => {
                if let Some(p) = key_to_cdp(&msg) {
                    page.execute(p)
                        .await
                        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
                }
            }
            BrowserClientMsg::Paste { .. } => {
                if let Some(p) = paste_to_cdp(&msg) {
                    page.execute(p)
                        .await
                        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
                }
            }
            BrowserClientMsg::ImeComposition { .. } => {
                if let Some(p) = ime_composition_to_cdp(&msg) {
                    page.execute(p)
                        .await
                        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
                }
            }
            BrowserClientMsg::ImeCommit { .. } => {
                if let Some(p) = ime_commit_to_cdp(&msg) {
                    page.execute(p)
                        .await
                        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
                }
            }
            // NOTE: non-Input variants are filtered upstream by the WS handler; the catch-all here is a contract assertion.
            _ => {}
        },
        PageCommand::Nav(n) => {
            match n {
                NavCommand::Navigate { url } => {
                    page.goto(url.as_str())
                        .await
                        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
                }
                NavCommand::Back => {
                    navigate_history(page, -1).await?;
                }
                NavCommand::Forward => {
                    navigate_history(page, 1).await?;
                }
                NavCommand::Reload => {
                    page.reload()
                        .await
                        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
                }
                NavCommand::Stop => {
                    page.execute(cdp_page::StopLoadingParams::default())
                        .await
                        .map_err(|e| BrowserError::Cdp(e.to_string()))?;
                }
            }
            // NOTE: Chromium auto-reapplies setDeviceMetricsOverride across
            // renderer swaps (emulation_handler.cc::UpdateDeviceEmulationState),
            // so we do NOT re-issue it here. The screencast stream subscription
            // is renderer-scoped and must be restarted so the new renderer's
            // VideoConsumer ships frames at the emulated viewport's bitmap size.
            if let Some((bitmap_w, bitmap_h)) = state.bitmap_dims() {
                restart_screencast(page, bitmap_w, bitmap_h).await?;
            }
        }
        PageCommand::OnMainFrameLoaded => {
            if let Some((bitmap_w, bitmap_h)) = state.bitmap_dims() {
                restart_screencast(page, bitmap_w, bitmap_h).await?;
            }
        }
        PageCommand::Resize {
            width,
            height,
            device_scale_factor,
        } => {
            let metrics = cdp_em::SetDeviceMetricsOverrideParams::builder()
                .width(width as i64)
                .height(height as i64)
                .device_scale_factor(device_scale_factor)
                .mobile(false)
                .build()
                .map_err(BrowserError::Cdp)?;
            page.execute(metrics)
                .await
                .map_err(|e| BrowserError::Cdp(e.to_string()))?;

            state.viewport = Some((width, height, device_scale_factor));
            let (bitmap_w, bitmap_h) = state.bitmap_dims().expect("just set viewport");
            restart_screencast(page, bitmap_w, bitmap_h).await?;

            snapshot_tx.send_modify(|snap| {
                Arc::make_mut(snap).viewport = Viewport { width, height };
            });
        }
        PageCommand::PauseScreencast => {
            page.execute(cdp_page::StopScreencastParams::default())
                .await
                .map_err(|e| BrowserError::Cdp(e.to_string()))?;
        }
        PageCommand::ResumeScreencast => {
            // NOTE: only resume if a viewport has been established. Otherwise
            // the stream would start at an unknown default size; the next
            // Resize command will start it correctly.
            if let Some((bitmap_w, bitmap_h)) = state.bitmap_dims() {
                restart_screencast(page, bitmap_w, bitmap_h).await?;
            }
        }
        PageCommand::GetSelection(reply) => {
            let v = page
                .evaluate("getSelection().toString()")
                .await
                .ok()
                .and_then(|r| r.into_value::<String>().ok())
                .unwrap_or_default();
            let _ = reply.send(v);
        }
        PageCommand::Close => {}
    }
    Ok(())
}

/// Navigate the page backward (`delta = -1`) or forward (`delta = 1`) by
/// fetching the navigation history and invoking `navigateToHistoryEntry`.
///
/// This is the correct CDP pattern for Back/Forward in chromiumoxide 0.7.0
/// because the library does not expose high-level `go_back`/`go_forward`
/// helpers — only the raw `Page.getNavigationHistory` +
/// `Page.navigateToHistoryEntry` pair.
async fn navigate_history(page: &chromiumoxide::Page, delta: i64) -> BrowserResult<()> {
    use chromiumoxide::cdp::browser_protocol::page as cdp_page;

    let history = page
        .execute(cdp_page::GetNavigationHistoryParams::default())
        .await
        .map_err(|e| BrowserError::Cdp(e.to_string()))?;

    let current = history.result.current_index;
    let target_index = current + delta;
    let entries = &history.result.entries;

    if target_index < 0 || target_index as usize >= entries.len() {
        return Ok(());
    }

    let entry_id = entries[target_index as usize].id;
    page.execute(cdp_page::NavigateToHistoryEntryParams::new(entry_id))
        .await
        .map_err(|e| BrowserError::Cdp(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{BrowserClientMsg, KeyKind, NavCommand};

    #[test]
    fn page_command_input_is_debug_printable() {
        let cmd = PageCommand::Input(BrowserClientMsg::Key {
            key_kind: KeyKind::Down,
            code: "KeyA".into(),
            key: "a".into(),
            text: None,
            modifiers: 0,
        });
        let s = format!("{cmd:?}");
        assert!(s.contains("Input"));
    }

    #[test]
    fn page_command_nav_is_debug_printable() {
        let cmd = PageCommand::Nav(NavCommand::Reload);
        let s = format!("{cmd:?}");
        assert!(s.contains("Nav"));
    }
}
