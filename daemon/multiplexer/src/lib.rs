//! In-memory multiplexer domain model: sessions, panes, activities, and the
//! layout cell tree. No I/O.

use std::collections::HashMap;

pub mod error;
pub mod session;

pub use error::{MultiplexerError, MultiplexerResult};
pub use session::{
    Activity, ActivityId, ActivityKind, BrowserProfile, Cell, CellId, CloseOutcome, CycleDirection,
    LayoutCellState, Pane, PaneCell, PaneDirection, PaneId, PaneState, ResizePaneOutcome, RootCell,
    Session, SessionDimensions, SessionId, SetActiveOutcome, Side, SplitCell, SplitOrientation,
    SwapOffset, SwapOutcome,
};

/// Backwards-compatible alias for the active-pane outcome.
pub type SetActivePaneOutcome = SetActiveOutcome;

/// Outcome of `set_session_dimensions`: whether the cached value changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetDimensionsOutcome {
    /// New value differed from the previous one.
    Applied,
    /// Same as before; caller can skip side effects.
    Unchanged,
}

/// In-memory domain model. Owns the multiplexer's session store and the
/// pane-owner index. Pure data — no PTY, no extension registry, no broadcast.
#[derive(Default, Clone)]
pub struct MultiplexerService {
    pub sessions: HashMap<SessionId, Session>,
    pub pane_owner_session: HashMap<PaneId, SessionId>,
    next_session_id: u32,
}

impl MultiplexerService {
    /// Borrow the Session for `id` and run `f` against it.
    pub fn with_session<R>(
        &mut self,
        id: &SessionId,
        f: impl FnOnce(&mut Session) -> R,
    ) -> Option<R> {
        self.sessions.get_mut(id).map(f)
    }

    /// Same as `with_session` but propagates `SessionNotFound`.
    pub fn with_session_or_404<R>(
        &mut self,
        id: &SessionId,
        f: impl FnOnce(&mut Session) -> MultiplexerResult<R>,
    ) -> MultiplexerResult<R> {
        match self.sessions.get_mut(id) {
            Some(s) => f(s),
            None => Err(MultiplexerError::SessionNotFound(*id)),
        }
    }

    /// Create a Session containing one initial Pane with one initial Terminal Activity.
    /// Returns `(SessionId, PaneId, ActivityId)`.
    pub fn create_session(&mut self, name: Option<String>) -> (SessionId, PaneId, ActivityId) {
        let session_id = SessionId(self.next_session_id);
        self.next_session_id = self
            .next_session_id
            .checked_add(1)
            .expect("SessionId u32 counter overflow");

        let pane_id = PaneId::new();
        let activity = Activity::terminal(ActivityId::new());
        let activity_id = activity.id.clone();
        let session_name = name.unwrap_or_else(|| format!("Session{}", self.sessions.len() + 1));
        let session =
            Session::new_with_initial(session_id, session_name, pane_id.clone(), activity);

        self.sessions.insert(session_id, session);
        self.pane_owner_session.insert(pane_id.clone(), session_id);

        (session_id, pane_id, activity_id)
    }

    /// Rename a Session in-place.
    pub fn rename_session(&mut self, sid: &SessionId, name: String) -> MultiplexerResult<()> {
        self.with_session_or_404(sid, |s| {
            s.rename(name);
            Ok(())
        })
    }

    /// Replace the cached cell-grid dimensions for `sid`.
    pub fn set_session_dimensions(
        &mut self,
        sid: &SessionId,
        cols: u16,
        rows: u16,
    ) -> MultiplexerResult<SetDimensionsOutcome> {
        self.with_session_or_404(sid, |s| {
            let next = SessionDimensions { cols, rows };
            if s.dimensions.as_ref() == Some(&next) {
                return Ok(SetDimensionsOutcome::Unchanged);
            }
            s.set_dimensions(cols, rows);
            Ok(SetDimensionsOutcome::Applied)
        })
    }

    /// Run the resize-pane algorithm.
    pub fn resize_pane(
        &mut self,
        sid: &SessionId,
        pane: &PaneId,
        direction: PaneDirection,
        amount: u16,
    ) -> MultiplexerResult<ResizePaneOutcome> {
        self.with_session_or_404(sid, |s| {
            let dims = s
                .dimensions
                .clone()
                .ok_or(MultiplexerError::SessionNotMeasured(*sid))?;
            Ok(crate::session::resize::resize_split_for_pane(
                &mut s.cells,
                &s.pane_to_cell,
                pane,
                direction,
                amount,
                dims.cols,
                dims.rows,
            ))
        })
    }

    /// Resolve which Session currently owns `pid`. Returns `PaneNotFound`
    /// when the pane has no recorded owner.
    pub fn lookup_pane_session(&self, pid: &PaneId) -> MultiplexerResult<SessionId> {
        self.pane_owner_session
            .get(pid)
            .copied()
            .ok_or_else(|| MultiplexerError::PaneNotFound(pid.clone()))
    }

    /// Remove a Session and return the activities and pane ids the caller must
    /// clean up.
    pub fn close_session_data(
        &mut self,
        sid: &SessionId,
    ) -> MultiplexerResult<(Vec<ActivityId>, Vec<PaneId>)> {
        let session = self
            .sessions
            .remove(sid)
            .ok_or(MultiplexerError::SessionNotFound(*sid))?;

        let activities = session.collect_activities_for_cleanup();
        let pane_ids: Vec<PaneId> = session.pane_ids().cloned().collect();

        for pid in &pane_ids {
            self.pane_owner_session.remove(pid);
        }
        Ok((activities, pane_ids))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_session_returns_initial_ids() {
        let mut svc = MultiplexerService::default();
        let (sid, pid, aid) = svc.create_session(None);
        assert_eq!(sid, SessionId(0));
        assert_eq!(svc.sessions.len(), 1);
        assert_eq!(svc.pane_owner_session.get(&pid), Some(&sid));
        let session = svc.sessions.get(&sid).expect("session present");
        let pane = session.pane(&pid).expect("pane present");
        assert!(pane.activity_ids().any(|a| a == &aid));
    }

    #[test]
    fn create_session_mints_monotonic_ids() {
        let mut svc = MultiplexerService::default();
        let (a, _, _) = svc.create_session(None);
        let (b, _, _) = svc.create_session(None);
        let (c, _, _) = svc.create_session(None);
        assert_eq!(a, SessionId(0));
        assert_eq!(b, SessionId(1));
        assert_eq!(c, SessionId(2));
    }

    #[test]
    fn set_session_dimensions_stores_values() {
        let mut svc = MultiplexerService::default();
        let (sid, _, _) = svc.create_session(None);
        let outcome = svc.set_session_dimensions(&sid, 120, 40).unwrap();
        assert_eq!(outcome, SetDimensionsOutcome::Applied);
        let dims = svc
            .with_session_or_404(&sid, |s| Ok::<_, MultiplexerError>(s.dimensions.clone()))
            .unwrap();
        assert_eq!(
            dims,
            Some(SessionDimensions {
                cols: 120,
                rows: 40
            })
        );
    }

    #[test]
    fn set_session_dimensions_returns_unchanged_when_same_value() {
        let mut svc = MultiplexerService::default();
        let (sid, _, _) = svc.create_session(None);
        let first = svc.set_session_dimensions(&sid, 120, 40).unwrap();
        assert_eq!(first, SetDimensionsOutcome::Applied);
        let second = svc.set_session_dimensions(&sid, 120, 40).unwrap();
        assert_eq!(second, SetDimensionsOutcome::Unchanged);
    }

    #[test]
    fn set_session_dimensions_unknown_session_returns_session_not_found() {
        let mut svc = MultiplexerService::default();
        let err = svc
            .set_session_dimensions(&SessionId(99), 80, 24)
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::SessionNotFound(_)));
    }

    #[test]
    fn resize_pane_returns_session_not_measured_when_dimensions_unset() {
        let mut svc = MultiplexerService::default();
        let (sid, pid, _aid) = svc.create_session(None);
        let err = svc
            .resize_pane(&sid, &pid, PaneDirection::Right, 1)
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::SessionNotMeasured(_)));
    }

    #[test]
    fn resize_pane_returns_no_op_when_single_pane_session() {
        let mut svc = MultiplexerService::default();
        let (sid, pid, _aid) = svc.create_session(None);
        svc.set_session_dimensions(&sid, 120, 40).unwrap();
        let outcome = svc
            .resize_pane(&sid, &pid, PaneDirection::Right, 1)
            .unwrap();
        assert!(matches!(outcome, ResizePaneOutcome::NoOp));
    }
}
