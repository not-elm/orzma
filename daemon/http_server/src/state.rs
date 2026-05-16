use crate::handlers::windows::panes::spawn_terminal::spawn_terminal_pty;
use crate::layout_broadcast::LayoutBroadcaster;
use crate::window_view::WindowView;
use crate::{HttpError, HttpResult};
use axum::extract::FromRef;
use ozmux_configs::OzmuxConfigs;
use ozmux_extension::ExtensionRegistry;
use ozmux_multiplexer::{
    Activity, ActivityId, ActivityKind, CycleDirection, MultiplexerError, MultiplexerResult,
    MultiplexerService, PaneDirection, PaneId, SessionId, SetActiveOutcome, SetActivePaneOutcome,
    Side, SplitOrientation, WindowId,
};
use ozmux_terminal::TerminalService;
use std::sync::Arc;

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
    /// Daemon-wide configuration loaded at startup (shortcuts, etc.).
    pub configs: Arc<OzmuxConfigs>,
}

impl AppState {
    /// Build an `AppState` wired to the supplied runtime services. This is the
    /// only sanctioned construction path outside tests — `Default` is
    /// intentionally not derived so callers cannot silently produce a state
    /// whose `TerminalService`, `ExtensionRegistry`, or `LayoutBroadcaster`
    /// are detached from the daemon's runtime root.
    pub fn new(
        terminal: TerminalService,
        extensions: ExtensionRegistry,
        layout_broadcast: LayoutBroadcaster,
        configs: Arc<OzmuxConfigs>,
    ) -> Self {
        Self {
            multiplexer: MultiplexerService::default(),
            terminal,
            extensions,
            layout_broadcast,
            configs,
        }
    }

    pub async fn activate_activity(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        aid: &ActivityId,
    ) -> HttpResult {
        let outcome = self
            .multiplexer
            .with_window_or_404(wid, |w| w.pane_mut(pid)?.set_active_activity(aid))
            .await?;
        if matches!(outcome, SetActiveOutcome::Changed) {
            self.publish_window_layout(wid).await;
        }
        Ok(())
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
            .with_window_or_404(wid, |w| -> MultiplexerResult<SetActiveOutcome> {
                let from = w.active_pane.clone();
                match w.pane_in_direction(&from, direction)? {
                    Some(target) => w.set_active_pane(&target),
                    None => Ok(SetActiveOutcome::Unchanged),
                }
            })
            .await?;
        if matches!(outcome, SetActiveOutcome::Changed) {
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
        if matches!(activity_kind, ActivityKind::Terminal)
            && let Err(spawn_err) = spawn_terminal_pty(self, wid, pid, &aid).await
        {
            if let Err(rollback_err) = self.rollback_added_activity(wid, pid, &aid).await {
                tracing::warn!(
                    error = %rollback_err,
                    %wid, %pid, %aid,
                    "failed to roll back added activity after PTY spawn failure"
                );
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
        let removed = self
            .multiplexer
            .with_window_or_404(wid, |w| w.pane_mut(pid)?.remove_activity(aid))
            .await?;

        match removed.kind {
            ActivityKind::Terminal => {
                let _ = self.terminal.kill(aid).await;
            }
            ActivityKind::Extension { .. } => {
                self.extensions.forget_activity(aid);
            }
        }

        self.publish_window_layout(wid).await;
        Ok(())
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
        if matches!(activity_kind, ActivityKind::Terminal)
            && let Err(spawn_err) =
                spawn_terminal_pty(self, wid, &new_pane_id, &new_activity_id).await
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
        for aid in &activities {
            self.extensions.forget_activity(aid);
        }
        for aid in &activities {
            let _ = self.terminal.kill(aid).await;
        }

        self.publish_window_layout(wid).await;
        Ok(())
    }

    /// Rename a Window and broadcast the new layout.
    pub async fn rename_window(&self, wid: &WindowId, name: String) -> HttpResult<()> {
        self.multiplexer.rename_window(wid, name).await?;
        self.publish_window_layout(wid).await;
        Ok(())
    }

    /// Close a Window: tear down its Panes/Activities and run runtime
    /// cleanup. Delegates the data half to `multiplexer.close_window_data`
    /// and then issues PTY kills, extension registry forgets, and a layout
    /// broadcast close.
    pub async fn close_window(&self, wid: &WindowId) -> MultiplexerResult<Vec<ActivityId>> {
        let (activities, pane_ids) = self.multiplexer.close_window_data(wid).await?;
        for pid in pane_ids {
            self.extensions.forget_pane(&pid);
        }
        for aid in &activities {
            let _ = self.terminal.kill(aid).await;
            self.extensions.forget_activity(aid);
        }
        self.layout_broadcast.close(wid);
        Ok(activities)
    }

    /// Delete a Session, cascading into every Window it owns.
    pub async fn delete_session(&self, sid: &SessionId) -> MultiplexerResult<Vec<ActivityId>> {
        let linked = self.multiplexer.take_session_windows(sid).await?;
        let mut activities = Vec::new();
        for wid in linked {
            activities.extend(self.close_window(&wid).await?);
        }
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
            .await
    }

    async fn rollback_split(&self, wid: &WindowId, new_pane_id: &PaneId) {
        // NOTE: spawn happens before publish, so the frontend never saw the new
        // pane — no layout re-broadcast is needed on rollback.
        let closed = self
            .multiplexer
            .with_window_or_404(wid, |w| w.close_pane(new_pane_id))
            .await
            .is_ok();
        if !closed {
            tracing::warn!(
                %new_pane_id,
                "split rollback failed to close pane after spawn failure"
            );
            return;
        }
        self.multiplexer.pane_owner_window.remove(new_pane_id);
    }

    /// Build the current Window layout snapshot under the Window lock and
    /// broadcast it. Used by every handler that mutates a Window and by the
    /// title-republish task.
    pub(crate) async fn publish_window_layout(&self, wid: &WindowId) {
        // NOTE: titles are snapshotted separately from the window state, so a
        // published view's title can be one title-change cycle stale. This is
        // benign — the next title change re-broadcasts the corrected view.
        let titles = self.terminal.all_titles().await;
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
