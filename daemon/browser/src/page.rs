//! Per-page actor. Owns a `chromiumoxide::Page` and serializes CDP calls so
//! the registry lock in `BrowserService` never holds across an `await`. One
//! actor per browser Activity.

use crate::error::{BrowserError, BrowserResult};
use crate::input::{
    ime_commit_to_cdp, ime_composition_to_cdp, key_to_cdp, mouse_to_cdp, paste_to_cdp, wheel_to_cdp,
};
use crate::wire::{BrowserClientMsg, NavCommand};
use tokio::sync::{mpsc, oneshot};

/// Command sent to a `PageActor` via its `mpsc::Sender`.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum PageCommand {
    /// Translate a wire input message and forward it to CDP.
    Input(BrowserClientMsg),
    /// Drive page navigation (Navigate/Back/Forward/Reload/Stop).
    Nav(NavCommand),
    /// Update the page's emulated viewport.
    Resize {
        /// Viewport width in CSS pixels.
        width: u32,
        /// Viewport height in CSS pixels.
        height: u32,
    },
    /// Pause screencast (CDP `Page.stopScreencast`).
    PauseScreencast,
    /// Resume screencast — request the bridge task to re-arm
    /// `Page.startScreencast`. The page actor itself does not start
    /// screencast directly; it signals the bridge owner.
    ResumeScreencast,
    /// Reply with the page's current selection text via `Runtime.evaluate`.
    GetSelection(oneshot::Sender<String>),
    /// Stop the actor; close the page.
    Close,
}

/// Run the actor loop. Consumes the `Page` and the `mpsc::Receiver` end of
/// the actor command channel. Returns when the channel closes or a `Close`
/// command arrives. The owning `BrowserService` task spawns this on a
/// dedicated tokio task per Activity.
#[allow(dead_code)]
pub(crate) async fn run(
    page: chromiumoxide::Page,
    mut rx: mpsc::Receiver<PageCommand>,
) -> BrowserResult<()> {
    while let Some(cmd) = rx.recv().await {
        if matches!(cmd, PageCommand::Close) {
            break;
        }
        if let Err(e) = handle(&page, cmd).await {
            tracing::warn!(error = %e, "page command failed");
        }
    }
    let _ = page.close().await;
    Ok(())
}

#[allow(dead_code)]
async fn handle(page: &chromiumoxide::Page, cmd: PageCommand) -> BrowserResult<()> {
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
        PageCommand::Nav(n) => match n {
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
        },
        PageCommand::Resize { width, height } => {
            // NOTE: device_scale_factor fixed at 1 per spec MVP — DPR
            // emulation deferred (ExperimentalDeprecated CDP API).
            let params = cdp_em::SetDeviceMetricsOverrideParams::builder()
                .width(width as i64)
                .height(height as i64)
                .device_scale_factor(1.0)
                .mobile(false)
                .build()
                .map_err(BrowserError::Cdp)?;
            page.execute(params)
                .await
                .map_err(|e| BrowserError::Cdp(e.to_string()))?;
        }
        PageCommand::PauseScreencast => {
            page.execute(cdp_page::StopScreencastParams::default())
                .await
                .map_err(|e| BrowserError::Cdp(e.to_string()))?;
        }
        PageCommand::ResumeScreencast => {
            // NOTE: the actor cannot restart screencast on its own — the
            // bridge task (`bridge::run`) owns the listener loop and the
            // ack ownership. Task 2.7 wires a signal channel from here to
            // the bridge; for now this is a no-op so the actor compiles
            // and the WS handler can already plumb the command.
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
#[allow(dead_code)]
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
