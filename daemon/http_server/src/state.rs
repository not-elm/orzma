use crate::activity_titles::ActivityTitles;
use crate::handlers::windows::panes::spawn_terminal::spawn_terminal_pty;
use crate::layout_broadcast::LayoutBroadcaster;
use crate::session_broadcast::SessionBroadcaster;
use crate::session_view::SessionView;
use crate::window_view::WindowView;
use crate::{HttpError, HttpResult};
use axum::extract::FromRef;
use ozmux_browser::cef_backend::CefBackend;
use ozmux_browser::cef_dispatcher::CefDispatcher;
use ozmux_browser::cef_registry::BrowserCefRegistry;
use ozmux_browser_cef_protocol::types::ActivityId as CefActivityId;
use ozmux_configs::OzmuxConfigs;
use ozmux_extension::ExtensionRegistry;
use ozmux_multiplexer::{
    Activity, ActivityId, ActivityKind, CycleDirection, MultiplexerError, MultiplexerResult,
    MultiplexerService, PaneDirection, PaneId, SessionId, SetActiveOutcome, SetActivePaneOutcome,
    Side, SplitOrientation, WindowId,
};
use ozmux_terminal::TerminalService;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Lightweight discriminant for `ActivityKind`, used by route-kind guards
/// to assert an Activity matches a specific kind before dispatching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKindDiscriminant {
    /// Terminal PTY activity.
    Terminal,
    /// Extension (iframe-served) activity.
    Extension,
    /// Browser activity.
    Browser,
}

/// Input bundle for [`AppState::split_pane`].
pub struct SplitInput {
    /// Id for the new pane (caller-supplied or server-generated).
    pub new_pane_id: PaneId,
    /// The activity to seat in the new pane.
    pub new_activity: Activity,
    /// Extension name when the activity kind is Extension.
    pub extension_name: Option<String>,
    /// Which side of the target pane the new pane appears on.
    pub side: Side,
    /// Axis along which to split.
    pub orientation: SplitOrientation,
}

/// Ids produced by a successful [`AppState::split_pane`].
pub struct SplitOutcome {
    /// Id of the newly-created pane.
    pub new_pane_id: PaneId,
    /// Id of the activity seated in the new pane.
    pub new_activity_id: ActivityId,
}

/// Input bundle for [`AppState::break_activity_to_pane`].
pub struct BreakActivityInput {
    /// Id for the new pane (caller-supplied or server-generated).
    pub new_pane_id: PaneId,
    /// Which side of the target pane the new pane appears on.
    pub side: Side,
    /// Axis along which to split.
    pub orientation: SplitOrientation,
}

#[derive(Clone)]
pub struct AppState {
    pub multiplexer: MultiplexerService,
    pub terminal: TerminalService,
    pub extensions: ExtensionRegistry,
    pub layout_broadcast: LayoutBroadcaster,
    /// Per-session WS broadcaster; fed by `publish_session_view`.
    pub session_broadcast: SessionBroadcaster,
    /// Daemon-wide configuration loaded at startup (shortcuts, etc.).
    pub configs: Arc<OzmuxConfigs>,
    /// Kind-agnostic per-activity title map. All activity kinds (terminal,
    /// browser, …) publish into this; consumers snapshot it for layout builds.
    pub titles: ActivityTitles,
    /// CEF-backed BrowserActivity registry.
    pub browser_cef: Arc<BrowserCefRegistry>,
    /// Dispatcher for the cef_host transport. Production wires
    /// `LiveCefDispatcher`; tests wire `StubCefDispatcher`. Plan 3 swaps the
    /// live impl for in-process.
    pub cef_host: Arc<dyn CefDispatcher>,
}

impl AppState {
    /// Build an `AppState` wired to the supplied runtime services. This is the
    /// only sanctioned construction path outside tests — `Default` is
    /// intentionally not derived so callers cannot silently produce a state
    /// whose `TerminalService`, `ExtensionRegistry`, or `LayoutBroadcaster`
    /// are detached from the daemon's runtime root.
    #[expect(
        clippy::too_many_arguments,
        reason = "AppState aggregates every daemon subsystem; threading them through a builder would just rename the noise"
    )]
    pub fn new(
        terminal: TerminalService,
        extensions: ExtensionRegistry,
        layout_broadcast: LayoutBroadcaster,
        session_broadcast: SessionBroadcaster,
        configs: Arc<OzmuxConfigs>,
        titles: ActivityTitles,
        cef_host: Arc<dyn CefDispatcher>,
        browser_cef: Arc<BrowserCefRegistry>,
    ) -> Self {
        Self {
            multiplexer: MultiplexerService::default(),
            terminal,
            extensions,
            layout_broadcast,
            session_broadcast,
            configs,
            titles,
            browser_cef,
            cef_host,
        }
    }

    pub async fn activate_activity(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        aid: &ActivityId,
    ) -> HttpResult {
        let prev_active = self
            .multiplexer
            .with_window(wid, |w| w.pane(pid).ok().map(|p| p.active_activity.clone()))
            .await
            .flatten();

        let outcome = self
            .multiplexer
            .with_window_or_404(wid, |w| w.pane_mut(pid)?.set_active_activity(aid))
            .await?;

        if matches!(outcome, SetActiveOutcome::Changed) {
            self.toggle_screencast_on_active_change(wid, pid, prev_active.as_ref(), aid)
                .await;
            self.publish_window_layout(wid).await;
        }
        Ok(())
    }

    /// Look up the `ActivityKindDiscriminant` of `aid` within `(wid, pid)`.
    /// Returns `None` when the activity is not present in that pane.
    async fn activity_kind_in_pane(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        aid: &ActivityId,
    ) -> Option<ActivityKindDiscriminant> {
        let kind = self
            .multiplexer
            .with_window(wid, |w| {
                w.pane(pid).ok()?.activity(aid).map(|a| match &a.kind {
                    ActivityKind::Terminal => ActivityKindDiscriminant::Terminal,
                    ActivityKind::Extension { .. } => ActivityKindDiscriminant::Extension,
                    ActivityKind::Browser { .. } => ActivityKindDiscriminant::Browser,
                })
            })
            .await??;
        Some(kind)
    }

    /// On active-activity change, pause screencast for the previous CEF-backed
    /// activity (Browser or Extension) and resume it for the next one. Both
    /// calls are missing-ok.
    pub(crate) async fn toggle_screencast_on_active_change(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        prev: Option<&ActivityId>,
        next: &ActivityId,
    ) {
        if let Some(prev) = prev
            && matches!(
                self.activity_kind_in_pane(wid, pid, prev).await,
                Some(ActivityKindDiscriminant::Browser | ActivityKindDiscriminant::Extension)
            )
        {
            let _ = self.cef_host.dispatch(
                ozmux_browser_cef_protocol::wire::HostCommand::PauseScreencast {
                    aid: CefActivityId(prev.to_string()),
                },
            );
        }
        if matches!(
            self.activity_kind_in_pane(wid, pid, next).await,
            Some(ActivityKindDiscriminant::Browser | ActivityKindDiscriminant::Extension)
        ) {
            let _ = self.cef_host.dispatch(
                ozmux_browser_cef_protocol::wire::HostCommand::ResumeScreencast {
                    aid: CefActivityId(next.to_string()),
                },
            );
        }
    }

    pub async fn cycle_active_activity(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        direction: CycleDirection,
    ) -> HttpResult {
        let outcome = self
            .multiplexer
            .with_window_or_404(wid, |w| w.pane_mut(pid)?.cycle_active_activity(direction))
            .await?;
        if matches!(outcome, SetActiveOutcome::Changed) {
            self.publish_window_layout(wid).await;
        }
        Ok(())
    }

    pub async fn activate_pane(&self, wid: &WindowId, pid: &PaneId) -> HttpResult {
        let outcome = self
            .multiplexer
            .with_window_or_404(wid, |w| {
                if w.panes.contains_key(pid) {
                    w.set_active_pane(pid)
                } else if self.multiplexer.pane_owner_window.contains_key(pid) {
                    Err(MultiplexerError::PaneNotInWindow {
                        window: w.id.clone(),
                        pane: pid.clone(),
                    })
                } else {
                    Err(MultiplexerError::PaneNotFound(pid.clone()))
                }
            })
            .await?;
        if matches!(outcome, SetActivePaneOutcome::Changed) {
            self.publish_window_layout(wid).await;
        }
        Ok(())
    }

    /// Move focus from the currently active pane to its geometric neighbor in
    /// `direction`. Resolves and activates inside one window-lock acquisition
    /// to avoid TOCTOU between lookup and set, and broadcasts the new layout
    /// only when the active pane actually changes.
    pub async fn focus_pane_direction(
        &self,
        wid: &WindowId,
        direction: PaneDirection,
    ) -> HttpResult {
        let outcome = self
            .multiplexer
            .with_window_or_404(wid, |window| -> MultiplexerResult<SetActiveOutcome> {
                let from = window.active_pane.clone();
                match window.pane_in_direction(&from, direction)? {
                    Some(target) => window.set_active_pane(&target),
                    None => Ok(SetActiveOutcome::Unchanged),
                }
            })
            .await?;
        if matches!(outcome, SetActiveOutcome::Changed) {
            self.publish_window_layout(wid).await;
        }
        Ok(())
    }

    /// Run the resize-pane algorithm via the multiplexer and publish a
    /// layout update only if the mutation was applied. Per spec §5.2,
    /// soft no-ops return 204 with no broadcast.
    pub async fn resize_pane(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        direction: ozmux_multiplexer::PaneDirection,
        amount: u16,
    ) -> HttpResult<()> {
        self.ensure_pane_in_window(wid, pid)?;
        let outcome = self
            .multiplexer
            .resize_pane(wid, pid, direction, amount)
            .await?;
        if matches!(outcome, ozmux_multiplexer::ResizePaneOutcome::Applied) {
            self.publish_window_layout(wid).await;
        }
        Ok(())
    }

    /// Swap the named pane's contents with its prev/next neighbor in the
    /// window's pane-index order. Publishes a layout update only when the
    /// mutation was applied (a single-pane window returns Ok and skips the
    /// broadcast, matching `resize_pane`'s soft-no-op behavior).
    pub async fn swap_pane(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        offset: ozmux_multiplexer::SwapOffset,
    ) -> HttpResult<()> {
        self.ensure_pane_in_window(wid, pid)?;
        let outcome = self
            .multiplexer
            .with_window_or_404(wid, |w| w.swap_pane(pid, offset))
            .await?;
        if matches!(outcome, ozmux_multiplexer::SwapOutcome::Swapped { .. }) {
            self.publish_window_layout(wid).await;
        }
        Ok(())
    }

    /// Add an Activity to a Pane and broadcast the new layout. For
    /// terminal-kind activities, also spawn the backing PTY; on spawn
    /// failure the activity record is rolled back before returning the
    /// error so the frontend never sees a half-state.
    pub async fn add_activity_to_pane(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        activity: Activity,
        extension_name: Option<&str>,
    ) -> HttpResult<ActivityId> {
        let aid = activity.id.clone();
        let activity_kind = activity.kind.clone();

        self.multiplexer
            .with_window_or_404(wid, |w| w.pane_mut(pid)?.add_activity(activity))
            .await?;

        if let Some(name) = extension_name {
            self.extensions.record_activity_owner(&aid, name);
        }

        // NOTE: PTY spawn must precede the layout publish so the frontend never
        // sees a terminal activity without a backing PTY.
        if let Err(spawn_err) =
            crate::provision::provision_activity_runtime(self, wid, pid, &aid, &activity_kind).await
        {
            if let Err(rollback_err) = self.rollback_added_activity(wid, pid, &aid).await {
                tracing::warn!(error = %rollback_err, %wid, %pid, %aid, "rollback failed");
            }
            return Err(spawn_err);
        }

        self.publish_window_layout(wid).await;
        Ok(aid)
    }

    /// Close a single Activity in a Pane: kill its PTY (terminal kind) or
    /// forget its extension registry entry (extension kind), then broadcast
    /// the new layout. Refuses to close the last activity via
    /// `Pane::remove_activity`'s built-in `CannotRemoveLastActivity` guard.
    pub async fn close_activity(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        aid: &ActivityId,
    ) -> HttpResult<()> {
        self.ensure_pane_in_window(wid, pid)?;
        self.multiplexer
            .with_window_or_404(wid, |w| w.pane_mut(pid)?.remove_activity(aid))
            .await?;

        // NOTE: every backend's close is idempotent + missing-ok; no kind dispatch required.
        let _ = self.terminal.kill(aid).await;
        self.extensions.forget_activity(aid);
        self.cef_close_activity(aid).await;

        self.publish_window_layout(wid).await;
        Ok(())
    }

    /// Drops the per-activity cef ring (no-op if the activity was never
    /// provisioned via cef) and tells cef_host to close its browser handle.
    /// Failure to reach cef_host is logged but never propagated.
    async fn cef_close_activity(&self, aid: &ActivityId) {
        let cef_aid = CefActivityId(aid.to_string());
        let backend = CefBackend {
            dispatcher: Arc::clone(&self.cef_host),
            registry: Arc::clone(&self.browser_cef),
        };
        backend.close(&cef_aid).await;
    }

    /// Split `target_pane_id` in `wid`, seat the activity from `input`, and
    /// spawn a PTY when the activity is Terminal. Rolls back on spawn failure.
    pub async fn split_pane(
        &self,
        wid: &WindowId,
        target_pane_id: &PaneId,
        input: SplitInput,
    ) -> HttpResult<SplitOutcome> {
        self.ensure_pane_in_window(wid, target_pane_id)?;
        let new_pane_id = input.new_pane_id.clone();
        let new_activity_id = input.new_activity.id.clone();
        let activity_kind = input.new_activity.kind.clone();

        self.multiplexer
            .with_window_or_404(wid, |w| -> MultiplexerResult<_> {
                w.split_pane(
                    target_pane_id,
                    new_pane_id.clone(),
                    input.new_activity,
                    input.side,
                    input.orientation,
                )
            })
            .await?;

        self.multiplexer
            .pane_owner_window
            .insert(new_pane_id.clone(), wid.clone());

        if let Some(name) = input.extension_name.as_deref() {
            self.extensions
                .record_pane_and_activity_owners(&new_pane_id, &new_activity_id, name);
        }

        // NOTE: PTY spawn must precede the layout publish so the frontend never
        // sees a terminal activity without a backing PTY.
        if let Err(spawn_err) = crate::provision::provision_activity_runtime(
            self,
            wid,
            &new_pane_id,
            &new_activity_id,
            &activity_kind,
        )
        .await
        {
            self.rollback_split(wid, &new_pane_id).await;
            return Err(spawn_err);
        }

        self.publish_window_layout(wid).await;
        Ok(SplitOutcome {
            new_pane_id,
            new_activity_id,
        })
    }

    /// Split `target_pane_id` and move the Activity `aid` from it into the
    /// new Pane. No PTY is spawned — the moved Activity keeps its existing
    /// one. Returns the id of the new Pane.
    pub async fn break_activity_to_pane(
        &self,
        wid: &WindowId,
        target_pane_id: &PaneId,
        aid: &ActivityId,
        input: BreakActivityInput,
    ) -> HttpResult<PaneId> {
        self.ensure_pane_in_window(wid, target_pane_id)?;
        let new_pane_id = input.new_pane_id;

        self.multiplexer
            .with_window_or_404(wid, |w| -> MultiplexerResult<_> {
                w.break_activity_to_pane(
                    target_pane_id,
                    aid,
                    new_pane_id.clone(),
                    input.side,
                    input.orientation,
                )
            })
            .await?;

        self.multiplexer
            .pane_owner_window
            .insert(new_pane_id.clone(), wid.clone());

        // NOTE: For an Extension-kind Activity the activity->owner registry row
        // is still valid (the ActivityId is unchanged); only the new pane->owner
        // row is missing. `activity_owner` returns `None` for terminal
        // activities, so this is a no-op in the common case.
        if let Some(name) = self.extensions.activity_owner(aid) {
            self.extensions.record_pane_owner(&new_pane_id, &name);
        }

        self.publish_window_layout(wid).await;
        Ok(new_pane_id)
    }

    /// Close a Pane: remove it from the cell tree, tear down extension
    /// registry rows and PTYs for each activity, and broadcast the new layout.
    pub async fn close_pane(&self, wid: &WindowId, pid: &PaneId) -> HttpResult<()> {
        self.ensure_pane_in_window(wid, pid)?;
        let activities = self
            .multiplexer
            .with_window_or_404(wid, |w| w.close_pane(pid))
            .await?;

        self.multiplexer.pane_owner_window.remove(pid);
        self.extensions.forget_pane(pid);
        // NOTE: every backend's close is idempotent + missing-ok; no kind dispatch required.
        for aid in &activities {
            let _ = self.terminal.kill(aid).await;
            self.extensions.forget_activity(aid);
            self.cef_close_activity(aid).await;
        }

        self.publish_window_layout(wid).await;
        Ok(())
    }

    /// Rename a Window. Broadcasts both the layout (window listeners)
    /// and the parent session view (status-bar listeners).
    pub async fn rename_window(&self, wid: &WindowId, name: String) -> HttpResult<()> {
        self.multiplexer.rename_window(wid, name).await?;
        self.publish_window_layout(wid).await;
        if let Some(sid) = self.parent_session(wid).await {
            self.publish_session_view(&sid).await;
        }
        Ok(())
    }

    /// Update the cached cell dimensions for `wid`. Validation of
    /// positive cols/rows is the handler's responsibility. Broadcasts a
    /// layout update only when the dimensions actually change.
    pub async fn set_window_dimensions(
        &self,
        wid: &WindowId,
        cols: u16,
        rows: u16,
    ) -> HttpResult<()> {
        let outcome = self
            .multiplexer
            .set_window_dimensions(wid, cols, rows)
            .await?;
        if matches!(outcome, ozmux_multiplexer::SetDimensionsOutcome::Applied) {
            self.publish_window_layout(wid).await;
        }
        Ok(())
    }

    /// Close a Window: tear down its Panes/Activities and run runtime
    /// cleanup. Delegates the data half to `multiplexer.close_window_data`
    /// and then issues PTY kills, extension registry forgets, a layout
    /// broadcast close, and a parent-session view publish.
    pub async fn close_window(&self, wid: &WindowId) -> MultiplexerResult<Vec<ActivityId>> {
        let parent = self.parent_session(wid).await;
        let (activities, pane_ids) = self.multiplexer.close_window_data(wid).await?;
        for pid in pane_ids {
            self.extensions.forget_pane(&pid);
        }
        // NOTE: every backend's close is idempotent + missing-ok; no kind dispatch required.
        for aid in &activities {
            let _ = self.terminal.kill(aid).await;
            self.extensions.forget_activity(aid);
            self.cef_close_activity(aid).await;
        }
        self.layout_broadcast.close(wid);
        if let Some(sid) = parent {
            self.publish_session_view(&sid).await;
        }
        Ok(activities)
    }

    /// Delete a Session, cascading into every Window it owns, and close
    /// its `session_broadcast` channel so any subscriber observes
    /// `RecvError::Closed`.
    pub async fn delete_session(&self, sid: &SessionId) -> MultiplexerResult<Vec<ActivityId>> {
        let linked = self.multiplexer.take_session_windows(sid).await?;
        let mut activities = Vec::new();
        for wid in linked {
            activities.extend(self.close_window(&wid).await?);
        }
        self.session_broadcast.close(sid);
        Ok(activities)
    }

    async fn rollback_added_activity(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        aid: &ActivityId,
    ) -> MultiplexerResult<()> {
        self.multiplexer
            .with_window_or_404(wid, |w| -> Result<(), MultiplexerError> {
                w.pane_mut(pid)?.remove_activity(aid).map(|_| ())
            })
            .await?;
        Ok(())
    }

    async fn rollback_split(&self, wid: &WindowId, new_pane_id: &PaneId) {
        // NOTE: spawn happens before publish, so the frontend never saw the new
        // pane — no layout re-broadcast is needed on rollback.
        let activities = self
            .multiplexer
            .with_window_or_404(wid, |w| w.close_pane(new_pane_id))
            .await;
        match activities {
            Ok(_aids) => {
                self.multiplexer.pane_owner_window.remove(new_pane_id);
            }
            Err(_) => {
                tracing::warn!(
                    %new_pane_id,
                    "split rollback failed to close pane after spawn failure"
                );
            }
        }
    }

    /// Build the current Window layout snapshot under the Window lock and
    /// broadcast it. Used by every handler that mutates a Window and by the
    /// title-republish task.
    pub(crate) async fn publish_window_layout(&self, wid: &WindowId) {
        // NOTE: titles are snapshotted separately from the window state, so a
        // published view's title can be one title-change cycle stale. This is
        // benign — the next title change re-broadcasts the corrected view.
        let titles = self.titles.snapshot().await;
        let _ = self
            .multiplexer
            .with_window(wid, |w| match WindowView::from_window(w, &titles) {
                Ok(view) => match serde_json::to_value(&view) {
                    Ok(value) => self.layout_broadcast.publish(wid, value),
                    Err(e) => tracing::warn!(error = %e, %wid, "skipped layout publish"),
                },
                Err(e) => tracing::warn!(error = %e, %wid, "skipped layout publish"),
            })
            .await;
    }

    /// Build a JSON snapshot of the current session view without publishing.
    /// Returns `None` if the session is missing or serialization fails.
    pub(crate) async fn snapshot_session_view(&self, sid: &SessionId) -> Option<serde_json::Value> {
        // NOTE: clone the session before dropping the sessions lock so we can
        // pass `&Session` to `SessionView::from_session` later. Per-window
        // locks must be acquired AFTER the sessions lock is released to
        // preserve the `sessions -> windows[wid]` lock-order invariant.
        let session = {
            let sessions = self.multiplexer.sessions.lock().await;
            match sessions.get(sid) {
                Ok(s) => s.clone(),
                Err(e) => {
                    tracing::warn!(error = %e, %sid, "skipped session snapshot");
                    return None;
                }
            }
        };

        let mut window_names: HashMap<WindowId, String> = HashMap::new();
        for wid in &session.linked_windows {
            if let Some(n) = self.multiplexer.with_window(wid, |w| w.name.clone()).await {
                window_names.insert(wid.clone(), n);
            }
        }

        let view = SessionView::from_session(&session, &window_names);
        match serde_json::to_value(&view) {
            Ok(value) => Some(value),
            Err(e) => {
                tracing::warn!(error = %e, %sid, "skipped session snapshot");
                None
            }
        }
    }

    /// Build the current session snapshot and broadcast it on
    /// `session_broadcast`. Used by every handler that mutates session-
    /// visible state. Missing-session errors are logged and dropped.
    pub(crate) async fn publish_session_view(&self, sid: &SessionId) {
        if let Some(value) = self.snapshot_session_view(sid).await {
            self.session_broadcast.publish(sid, value);
        }
    }

    /// Promote `wid` to its session's active window and broadcast the
    /// updated session view. Returns `WindowNotFound` /
    /// `WindowNotAttachedToSession` from the underlying multiplexer.
    pub async fn select_active_window(&self, wid: &WindowId) -> MultiplexerResult<()> {
        self.multiplexer.select_active_window(wid).await?;
        if let Some(sid) = self.parent_session(wid).await {
            self.publish_session_view(&sid).await;
        }
        Ok(())
    }

    /// Create a session and broadcast the resulting view.
    /// Returns the new session id.
    pub async fn create_session(&self, name: Option<String>) -> SessionId {
        let sid = self.multiplexer.create_session(name).await;
        self.publish_session_view(&sid).await;
        sid
    }

    /// Create a window attached to `session_id` (when given), spawn the
    /// PTY for its bootstrap terminal activity, make it the active
    /// window, and publish the parent `SessionView`.
    ///
    /// On PTY-spawn or active-window-selection failure the half-created
    /// window is rolled back via `close_window` and the original error
    /// is returned. A failing rollback is logged and the original error
    /// still wins.
    pub async fn create_window(
        &self,
        session_id: Option<&SessionId>,
        name: Option<String>,
    ) -> HttpResult<(WindowId, PaneId, ActivityId)> {
        let (wid, pid, aid) = self.multiplexer.create_window(session_id, name).await?;
        if let Err(spawn_err) = spawn_terminal_pty(self, &wid, &pid, &aid, None).await {
            self.rollback_window(&wid, "PTY spawn failure").await;
            return Err(spawn_err);
        }
        if session_id.is_some()
            && let Err(select_err) = self.select_active_window(&wid).await
        {
            self.rollback_window(&wid, "active-window selection failure")
                .await;
            return Err(select_err.into());
        }
        Ok((wid, pid, aid))
    }

    /// Provision a brand-new Session with an attached Window+Pane+terminal
    /// Activity and its PTY in a single atomic step, then broadcast the
    /// resulting `SessionView`.
    ///
    /// Used by both `serve()`'s bootstrap and the `POST /sessions` HTTP
    /// handler so the same provisioning lives in one place.
    ///
    /// On PTY-spawn failure the partially-created session/window are rolled
    /// back via `rollback_window` + `delete_session` (which also closes the
    /// session broadcast) before the original error is returned.
    ///
    /// `cwd` is the initial working directory for the spawned shell; `None`
    /// inherits the daemon process's CWD.
    pub async fn provision_session_with_activity(
        &self,
        name: Option<String>,
        cwd: Option<&Path>,
    ) -> HttpResult<(SessionId, WindowId, PaneId, ActivityId)> {
        let sid = self.multiplexer.create_session(name).await;

        let (wid, pid, aid) = match self.multiplexer.create_window(Some(&sid), None).await {
            Ok(triple) => triple,
            Err(e) => {
                self.rollback_session(&sid).await;
                return Err(e.into());
            }
        };

        if let Err(spawn_err) = spawn_terminal_pty(self, &wid, &pid, &aid, cwd).await {
            self.rollback_window(&wid, "PTY spawn failure").await;
            self.rollback_session(&sid).await;
            return Err(spawn_err);
        }

        self.publish_session_view(&sid).await;
        Ok((sid, wid, pid, aid))
    }

    /// Rename a session and broadcast the updated view.
    pub async fn rename_session(&self, sid: &SessionId, name: String) -> MultiplexerResult<()> {
        self.multiplexer.rename_session(sid, name).await?;
        self.publish_session_view(sid).await;
        Ok(())
    }

    /// Walk every session looking for one whose `linked_windows` contains
    /// `wid`. Returns the owning `SessionId` if found.
    pub(crate) async fn parent_session(&self, wid: &WindowId) -> Option<SessionId> {
        let sessions = self.multiplexer.sessions.lock().await;
        for (sid, session) in sessions.iter() {
            if session.linked_windows.contains(wid) {
                return Some(sid.clone());
            }
        }
        None
    }

    /// Combined membership check for `/windows/:wid/panes/:pid/activities/:aid/*`
    /// that also returns the resolved `Activity`. Callers like `iframe_serve`
    /// need both the validation and the activity metadata; doing them in one
    /// helper avoids a second Window lock acquisition.
    pub(crate) async fn ensure_activity_in_pane_in_window_and_fetch(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        aid: &ActivityId,
    ) -> HttpResult<Activity> {
        self.ensure_pane_in_window(wid, pid)?;
        let activity = self
            .multiplexer
            .with_window(wid, |w| w.pane(pid).map(|p| p.activity(aid).cloned()))
            .await
            .ok_or_else(|| HttpError::Session(MultiplexerError::WindowNotFound(wid.clone())))??
            .ok_or_else(|| {
                HttpError::Session(MultiplexerError::ActivityNotInPane {
                    pane: pid.clone(),
                    activity: aid.clone(),
                })
            })?;
        Ok(activity)
    }

    /// Membership-only variant for handlers that don't need the Activity
    /// payload (terminal WS, handlers WS).
    pub(crate) async fn ensure_activity_in_pane_in_window(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        aid: &ActivityId,
    ) -> HttpResult<()> {
        let _ = self
            .ensure_activity_in_pane_in_window_and_fetch(wid, pid, aid)
            .await?;
        Ok(())
    }

    /// Like [`Self::ensure_activity_in_pane_in_window_and_fetch`], but also
    /// asserts that the activity's kind matches `want`. Returns
    /// [`HttpError::ActivityKindMismatch`] when the kinds differ.
    pub(crate) async fn ensure_activity_kind(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        aid: &ActivityId,
        want: ActivityKindDiscriminant,
    ) -> HttpResult<Activity> {
        let activity = self
            .ensure_activity_in_pane_in_window_and_fetch(wid, pid, aid)
            .await?;
        let got = match &activity.kind {
            ActivityKind::Terminal => ActivityKindDiscriminant::Terminal,
            ActivityKind::Extension { .. } => ActivityKindDiscriminant::Extension,
            ActivityKind::Browser { .. } => ActivityKindDiscriminant::Browser,
        };
        if got != want {
            return Err(HttpError::ActivityKindMismatch {
                aid: aid.clone(),
                want,
                got,
            });
        }
        Ok(activity)
    }

    async fn rollback_session(&self, sid: &SessionId) {
        if let Err(rb) = self.delete_session(sid).await {
            tracing::warn!(error = %rb, %sid, "rollback delete_session failed");
        }
    }

    /// Tear down a half-created window after a `create_window` step failed.
    /// A failing teardown is logged; the caller still returns the original
    /// error.
    async fn rollback_window(&self, wid: &WindowId, context: &str) {
        if let Err(rollback_err) = self.close_window(wid).await {
            tracing::warn!(
                error = %rollback_err,
                %wid,
                "failed to roll back window after {context}"
            );
        }
    }

    /// Validate that `pid` lives inside `wid`. Returns `PaneNotFound` when the
    /// pane has no owner and `PaneNotInWindow` when it lives in a different
    /// Window. Used by every URL of shape `/windows/:wid/panes/:pid/*`.
    fn ensure_pane_in_window(&self, wid: &WindowId, pid: &PaneId) -> HttpResult<()> {
        let actual = self.multiplexer.lookup_pane_window(pid)?;
        if &actual != wid {
            return Err(HttpError::Session(MultiplexerError::PaneNotInWindow {
                window: wid.clone(),
                pane: pid.clone(),
            }));
        }
        Ok(())
    }
}

impl FromRef<AppState> for TerminalService {
    fn from_ref(input: &AppState) -> Self {
        input.terminal.clone()
    }
}

impl FromRef<AppState> for ExtensionRegistry {
    fn from_ref(input: &AppState) -> Self {
        input.extensions.clone()
    }
}

impl FromRef<AppState> for LayoutBroadcaster {
    fn from_ref(input: &AppState) -> Self {
        input.layout_broadcast.clone()
    }
}

impl FromRef<AppState> for MultiplexerService {
    fn from_ref(input: &AppState) -> Self {
        input.multiplexer.clone()
    }
}

impl FromRef<AppState> for Arc<OzmuxConfigs> {
    fn from_ref(input: &AppState) -> Self {
        Arc::clone(&input.configs)
    }
}

impl FromRef<AppState> for Arc<BrowserCefRegistry> {
    fn from_ref(input: &AppState) -> Self {
        Arc::clone(&input.browser_cef)
    }
}

impl FromRef<AppState> for Arc<dyn CefDispatcher> {
    fn from_ref(input: &AppState) -> Self {
        Arc::clone(&input.cef_host)
    }
}

#[cfg(test)]
mod session_publish_tests {
    use crate::test_helpers::fresh_state;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn publish_session_view_delivers_to_subscriber() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(Some("s".into())).await;
        let (_wid, _, _) = state
            .multiplexer
            .create_window(Some(&sid), Some("w".into()))
            .await
            .unwrap();

        let mut rx = state.session_broadcast.subscribe_or_create(&sid);
        state.publish_session_view(&sid).await;

        let view = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["name"].as_str(), Some("s"));
        assert_eq!(view["windows"][0]["name"].as_str(), Some("w"));
    }

    #[tokio::test]
    async fn publish_session_view_unknown_sid_logs_warn_no_panic() {
        let state = fresh_state();
        state
            .publish_session_view(&ozmux_multiplexer::SessionId::new())
            .await;
    }

    #[tokio::test]
    async fn parent_session_finds_owner() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(None).await;
        let (wid, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let parent = state.parent_session(&wid).await;
        assert_eq!(parent.as_ref(), Some(&sid));
    }

    #[tokio::test]
    async fn parent_session_returns_none_for_orphan() {
        let state = fresh_state();
        let (wid, _, _) = state.multiplexer.create_window(None, None).await.unwrap();
        let parent = state.parent_session(&wid).await;
        assert_eq!(parent, None);
    }

    #[tokio::test]
    async fn close_window_publishes_session_view_with_window_removed() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(None).await;
        let (wid_a, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let (_wid_b, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let mut rx = state.session_broadcast.subscribe_or_create(&sid);

        state.close_window(&wid_a).await.unwrap();

        let view = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["windows"].as_array().map(|a| a.len()), Some(1));
    }

    #[tokio::test]
    async fn delete_session_closes_session_broadcast() {
        use tokio::sync::broadcast::error::RecvError;
        let state = fresh_state();
        let sid = state.multiplexer.create_session(None).await;
        let mut rx = state.session_broadcast.subscribe_or_create(&sid);

        state.delete_session(&sid).await.unwrap();

        let err = rx.recv().await.expect_err("expected closed");
        assert!(matches!(err, RecvError::Closed));
    }

    #[tokio::test]
    async fn rename_window_publishes_session_view() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(None).await;
        let (wid, _, _) = state
            .multiplexer
            .create_window(Some(&sid), Some("orig".into()))
            .await
            .unwrap();
        let mut rx = state.session_broadcast.subscribe_or_create(&sid);

        state.rename_window(&wid, "renamed".into()).await.unwrap();

        // NOTE: `session_broadcast` is a separate channel from `layout_broadcast`,
        // so this receiver only sees the session-view publish.
        let view = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["windows"][0]["name"].as_str(), Some("renamed"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers;

    #[tokio::test]
    async fn ensure_activity_kind_rejects_mismatched_kind() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, term_aid) = test_helpers::bootstrap_default(&state).await;
        let err = state
            .ensure_activity_kind(&wid, &pid, &term_aid, ActivityKindDiscriminant::Browser)
            .await
            .expect_err("must reject mismatched kind");
        assert!(matches!(err, HttpError::ActivityKindMismatch { .. }));
    }
}

#[cfg(test)]
mod provision_session_with_activity_tests {
    use crate::test_helpers::fresh_state;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn provision_creates_full_chain_and_publishes() {
        let state = fresh_state();
        let (sid, wid, pid, aid) = state
            .provision_session_with_activity(Some("s1".into()), None)
            .await
            .expect("provision succeeded");

        assert!(!sid.as_ref().is_empty());
        assert!(!wid.as_ref().is_empty());
        assert!(!pid.as_ref().is_empty());
        assert!(!aid.as_ref().is_empty());

        let mut rx = state.session_broadcast.subscribe_or_create(&sid);
        state.publish_session_view(&sid).await;
        let view = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["name"].as_str(), Some("s1"));
        assert_eq!(view["windows"].as_array().map(|a| a.len()), Some(1));
    }

    #[tokio::test]
    async fn provision_rolls_back_on_pty_spawn_failure() {
        let state = fresh_state();
        state.terminal.inject_next_spawn_failure();

        state
            .provision_session_with_activity(Some("rollback".into()), None)
            .await
            .expect_err("expected PTY spawn to fail");

        assert_eq!(
            state.multiplexer.windows.len(),
            0,
            "the half-created window must be rolled back"
        );
        let sessions = state.multiplexer.sessions.lock().await;
        assert_eq!(
            sessions.len(),
            0,
            "the half-created session must be rolled back"
        );
    }

    #[tokio::test]
    async fn concurrent_provision_and_delete_does_not_deadlock() {
        let state = fresh_state();
        let (existing_sid, _, _, _) = state
            .provision_session_with_activity(Some("victim".into()), None)
            .await
            .unwrap();

        let state_a = state.clone();
        let state_b = state.clone();
        let victim = existing_sid.clone();

        let run = async {
            let h_new = tokio::spawn(async move {
                state_a
                    .provision_session_with_activity(Some("new".into()), None)
                    .await
            });
            let h_del = tokio::spawn(async move { state_b.delete_session(&victim).await });
            let _ = h_new.await.unwrap();
            let _ = h_del.await.unwrap();
        };

        timeout(Duration::from_secs(3), run)
            .await
            .expect("provision + delete deadlocked");
    }
}

#[cfg(test)]
mod create_window_tests {
    use crate::test_helpers::fresh_state;

    #[tokio::test]
    async fn create_window_spawns_pty_and_activates_window() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(None).await;
        let (wid, _pid, aid) = state.create_window(Some(&sid), None).await.unwrap();
        assert!(
            state.terminal.subscriber_count(&aid).await.is_some(),
            "a PTY must be spawned for the new window's bootstrap activity"
        );
        let sessions = state.multiplexer.sessions.lock().await;
        assert_eq!(
            sessions.get(&sid).unwrap().active_window.as_ref(),
            Some(&wid),
            "the new window must become the active window"
        );
    }

    #[tokio::test]
    async fn create_window_rolls_back_on_pty_spawn_failure() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(None).await;
        state.terminal.inject_next_spawn_failure();
        let result = state.create_window(Some(&sid), None).await;
        assert!(result.is_err(), "PTY spawn failure must propagate as Err");
        assert_eq!(
            state.multiplexer.windows.len(),
            0,
            "the half-created window must be rolled back"
        );
        let sessions = state.multiplexer.sessions.lock().await;
        assert!(
            sessions.get(&sid).unwrap().linked_windows.is_empty(),
            "the rolled-back window must be detached from the session"
        );
    }
}
