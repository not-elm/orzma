//! In-memory multiplexer domain model: sessions, windows, panes, activities,
//! and the layout cell tree. No I/O.

use std::collections::HashMap;

pub mod error;
pub mod session;
pub mod window;

pub use error::{MultiplexerError, MultiplexerResult};
pub use session::{Session, SessionId, SessionState};
pub use window::resize::ResizePaneOutcome;
pub use window::{
    Activity, ActivityId, ActivityKind, BrowserProfile, Cell, CellId, CloseOutcome, CycleDirection,
    LayoutCellState, Pane, PaneCell, PaneDirection, PaneId, PaneState, RootCell, SetActiveOutcome,
    Side, SplitCell, SplitOrientation, SwapOffset, SwapOutcome, Window, WindowDimensions, WindowId,
    WindowState,
};

/// Backwards-compatible alias for the active-pane outcome. Use
/// `SetActiveOutcome` directly in new code.
pub type SetActivePaneOutcome = SetActiveOutcome;

/// Outcome of `set_window_dimensions`: whether the cached value changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetDimensionsOutcome {
    /// New value differed from the previous one.
    Applied,
    /// Same as before; caller can skip side effects.
    Unchanged,
}

/// In-memory domain model. Owns the multiplexer's three stores (sessions,
/// windows, pane_owner_window index). Pure data — no PTY, no extension
/// registry, no layout broadcast.
#[derive(Default, Clone)]
pub struct MultiplexerService {
    pub sessions: SessionState,
    pub windows: HashMap<WindowId, Window>,
    pub pane_owner_window: HashMap<PaneId, WindowId>,
}

impl MultiplexerService {
    /// Borrow the Window for `id` and run `f` against it.
    pub fn with_window<R>(&mut self, id: &WindowId, f: impl FnOnce(&mut Window) -> R) -> Option<R> {
        self.windows.get_mut(id).map(f)
    }

    /// Same as `with_window` but propagates `WindowNotFound`.
    pub fn with_window_or_404<R>(
        &mut self,
        id: &WindowId,
        f: impl FnOnce(&mut Window) -> MultiplexerResult<R>,
    ) -> MultiplexerResult<R> {
        match self.windows.get_mut(id) {
            Some(w) => f(w),
            None => Err(MultiplexerError::WindowNotFound(id.clone())),
        }
    }

    /// Create a Session and register it.
    pub fn create_session(&mut self, name: Option<String>) -> SessionId {
        let session_id = SessionId::new();
        let session_name = name.unwrap_or_else(|| format!("Session{}", self.sessions.len() + 1));
        self.sessions.register(
            session_id.clone(),
            Session::empty(session_id.clone(), session_name),
        );
        session_id
    }

    /// Create a Window optionally attached to a Session.
    pub fn create_window(
        &mut self,
        session_id: Option<&SessionId>,
        name: Option<String>,
    ) -> MultiplexerResult<(WindowId, PaneId, ActivityId)> {
        if let Some(sid) = session_id {
            self.sessions.get(sid)?;
        }

        let window_id = WindowId::new();
        let pane_id = PaneId::new();
        let activity = Activity::terminal(ActivityId::new());
        let activity_id = activity.id.clone();
        let window_name = name.unwrap_or_else(|| format!("Window{}", self.windows.len() + 1));
        let window =
            Window::new_with_initial(window_id.clone(), window_name, pane_id.clone(), activity);

        self.windows.insert(window_id.clone(), window);
        self.pane_owner_window
            .insert(pane_id.clone(), window_id.clone());

        if let Some(sid) = session_id {
            let session = self.sessions.get_mut(sid)?;
            session.attach_window(window_id.clone());
        }

        Ok((window_id, pane_id, activity_id))
    }

    /// Resolve the currently focused pane of `sid`: returns the active
    /// `WindowId` of the session and the active `PaneId` of that window.
    /// Distinct errors are returned for "session missing" vs "session has
    /// no active window" so callers can log accurately.
    pub fn active_pane_of_session(&self, sid: &SessionId) -> MultiplexerResult<(WindowId, PaneId)> {
        let session = self.sessions.get(sid)?;
        let wid = session
            .active_window
            .clone()
            .ok_or_else(|| MultiplexerError::SessionHasNoActiveWindow(sid.clone()))?;
        let window = self
            .windows
            .get(&wid)
            .ok_or_else(|| MultiplexerError::WindowNotFound(wid.clone()))?;
        Ok((wid, window.active_pane.clone()))
    }

    /// Rename a Window in-place.
    pub fn rename_window(&mut self, wid: &WindowId, name: String) -> MultiplexerResult<()> {
        self.with_window_or_404(wid, |w| {
            w.rename(name);
            Ok(())
        })
    }

    /// Replace the cached cell-grid dimensions for `wid`.
    pub fn set_window_dimensions(
        &mut self,
        wid: &WindowId,
        cols: u16,
        rows: u16,
    ) -> MultiplexerResult<SetDimensionsOutcome> {
        self.with_window_or_404(wid, |w| {
            let next = WindowDimensions { cols, rows };
            if w.dimensions.as_ref() == Some(&next) {
                return Ok(SetDimensionsOutcome::Unchanged);
            }
            w.set_dimensions(cols, rows);
            Ok(SetDimensionsOutcome::Applied)
        })
    }

    /// Run the resize-pane algorithm.
    pub fn resize_pane(
        &mut self,
        wid: &WindowId,
        pane: &PaneId,
        direction: PaneDirection,
        amount: u16,
    ) -> MultiplexerResult<ResizePaneOutcome> {
        self.with_window_or_404(wid, |w| {
            let dims = w
                .dimensions
                .clone()
                .ok_or_else(|| MultiplexerError::WindowNotMeasured(wid.clone()))?;
            Ok(crate::window::resize::resize_split_for_pane(
                &mut w.cells,
                &w.pane_to_cell,
                pane,
                direction,
                amount,
                dims.cols,
                dims.rows,
            ))
        })
    }

    /// Rename a Session.
    pub fn rename_session(&mut self, sid: &SessionId, name: String) -> MultiplexerResult<()> {
        let session = self.sessions.get_mut(sid)?;
        session.rename(name);
        Ok(())
    }

    /// Promote `wid` to the active window of whichever Session owns it.
    pub fn select_active_window(&mut self, wid: &WindowId) -> MultiplexerResult<()> {
        if !self.windows.contains_key(wid) {
            return Err(MultiplexerError::WindowNotFound(wid.clone()));
        }
        for (_, session) in self.sessions.iter_mut() {
            if session.linked_windows.contains(wid) {
                session.active_window = Some(wid.clone());
                return Ok(());
            }
        }
        Err(MultiplexerError::WindowNotAttachedToSession(wid.clone()))
    }

    /// Cycle the active window of `sid` by `direction`. Thin delegate
    /// to `Session::cycle_active_window` after resolving the session.
    pub fn cycle_active_window(
        &mut self,
        sid: &SessionId,
        direction: CycleDirection,
    ) -> MultiplexerResult<SetActiveOutcome> {
        self.sessions.get_mut(sid)?.cycle_active_window(direction)
    }

    /// Resolve which Window currently owns `pid`. Returns `PaneNotFound`
    /// when the pane has no recorded owner.
    pub fn lookup_pane_window(&self, pid: &PaneId) -> MultiplexerResult<WindowId> {
        self.pane_owner_window
            .get(pid)
            .cloned()
            .ok_or_else(|| MultiplexerError::PaneNotFound(pid.clone()))
    }

    /// Remove a Window and detach it from any owning Session. Returns the
    /// activities and pane ids the caller must clean up.
    pub fn close_window_data(
        &mut self,
        wid: &WindowId,
    ) -> MultiplexerResult<(Vec<ActivityId>, Vec<PaneId>)> {
        let win = self
            .windows
            .remove(wid)
            .ok_or_else(|| MultiplexerError::WindowNotFound(wid.clone()))?;

        let activities = win.collect_activities_for_cleanup();
        let pane_ids: Vec<PaneId> = win.pane_ids().cloned().collect();

        for (_, session) in self.sessions.iter_mut() {
            session.detach_window(wid);
        }

        for pid in &pane_ids {
            self.pane_owner_window.remove(pid);
        }
        Ok((activities, pane_ids))
    }

    /// Remove a Session and return the WindowIds that were attached to it.
    pub fn take_session_windows(&mut self, sid: &SessionId) -> MultiplexerResult<Vec<WindowId>> {
        let session = self.sessions.remove(sid)?;
        Ok(session.linked_windows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_window_dimensions_stores_values() {
        let mut svc = MultiplexerService::default();
        let (wid, _, _) = svc.create_window(None, None).unwrap();
        let outcome = svc.set_window_dimensions(&wid, 120, 40).unwrap();
        assert_eq!(outcome, SetDimensionsOutcome::Applied);
        let dims = svc
            .with_window_or_404(&wid, |w| Ok::<_, MultiplexerError>(w.dimensions.clone()))
            .unwrap();
        assert_eq!(
            dims,
            Some(crate::WindowDimensions {
                cols: 120,
                rows: 40
            })
        );
    }

    #[test]
    fn set_window_dimensions_returns_unchanged_when_same_value() {
        let mut svc = MultiplexerService::default();
        let (wid, _, _) = svc.create_window(None, None).unwrap();
        let first = svc.set_window_dimensions(&wid, 120, 40).unwrap();
        assert_eq!(first, SetDimensionsOutcome::Applied);
        let second = svc.set_window_dimensions(&wid, 120, 40).unwrap();
        assert_eq!(second, SetDimensionsOutcome::Unchanged);
    }

    #[test]
    fn set_window_dimensions_unknown_window_returns_window_not_found() {
        let mut svc = MultiplexerService::default();
        let err = svc
            .set_window_dimensions(&WindowId::new(), 80, 24)
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::WindowNotFound(_)));
    }

    #[test]
    fn resize_pane_returns_window_not_measured_when_dimensions_unset() {
        let mut svc = MultiplexerService::default();
        let (wid, pid, _aid) = svc.create_window(None, None).unwrap();
        let err = svc
            .resize_pane(&wid, &pid, PaneDirection::Right, 1)
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::WindowNotMeasured(_)));
    }

    #[test]
    fn resize_pane_returns_no_op_when_single_pane_window() {
        let mut svc = MultiplexerService::default();
        let (wid, pid, _aid) = svc.create_window(None, None).unwrap();
        svc.set_window_dimensions(&wid, 120, 40).unwrap();
        let outcome = svc
            .resize_pane(&wid, &pid, PaneDirection::Right, 1)
            .unwrap();
        assert!(matches!(outcome, ResizePaneOutcome::NoOp));
    }
}
