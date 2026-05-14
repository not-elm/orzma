use crate::HttpResult;
use crate::layout_broadcast::LayoutBroadcaster;
use crate::window_view::WindowView;
use axum::extract::FromRef;
use ozmux_configs::OzmuxConfigs;
use ozmux_extension::ExtensionRegistry;
use ozmux_multiplexer::{
    Activity, ActivityId, ActivityKind, MultiplexerError, MultiplexerResult, MultiplexerService,
    PaneId, SessionId, SetActiveOutcome, SetActivePaneOutcome, Side, SplitOrientation, WindowId,
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

    pub async fn add_activity_to_pane(
        &self,
        wid: &WindowId,
        pid: &PaneId,
        activity: Activity,
        extension_name: Option<&str>,
    ) -> MultiplexerResult<ActivityId> {
        let aid = activity.id.clone();
        self.multiplexer
            .with_window_or_404(wid, |w| w.pane_mut(pid)?.add_activity(activity))
            .await?;
        if let Some(name) = extension_name {
            self.extensions.record_activity_owner(&aid, name);
        }
        Ok(aid)
    }

    /// Split `target_pane_id` in `wid`, seat the activity from `input`, and
    /// spawn a PTY when the activity is Terminal. Rolls back on spawn failure.
    pub async fn split_pane(
        &self,
        wid: &WindowId,
        target_pane_id: &PaneId,
        input: SplitInput,
    ) -> HttpResult<SplitOutcome> {
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
        // sees a terminal activity without a backing PTY (mirrors the invariant
        // in close_activity / add_activity_to_pane).
        if matches!(activity_kind, ActivityKind::Terminal) {
            if let Err(spawn_err) =
                crate::handlers::windows::panes::spawn_terminal::spawn_terminal_pty(
                    self,
                    wid,
                    &new_pane_id,
                    &new_activity_id,
                )
                .await
            {
                self.rollback_split(wid, &new_pane_id).await;
                return Err(spawn_err);
            }
        }

        self.publish_window_layout(wid).await;
        Ok(SplitOutcome {
            new_pane_id,
            new_activity_id,
        })
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

    /// Close a Pane: remove it from the cell tree, tear down extension
    /// registry rows and PTYs for each activity, and broadcast the new layout.
    pub async fn close_pane(&self, wid: &WindowId, pid: &PaneId) -> HttpResult<()> {
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

    /// Build the current Window layout snapshot under the Window lock and
    /// broadcast it. Used by every handler that mutates a Window.
    async fn publish_window_layout(&self, wid: &WindowId) {
        let _ = self
            .multiplexer
            .with_window(wid, |w| match WindowView::from_window(w) {
                Ok(view) => match serde_json::to_value(&view) {
                    Ok(value) => self.layout_broadcast.publish(wid, value),
                    Err(e) => tracing::warn!(error = %e, %wid, "skipped layout publish"),
                },
                Err(e) => tracing::warn!(error = %e, %wid, "skipped layout publish"),
            })
            .await;
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
