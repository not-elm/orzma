use crate::window::{CycleDirection, SetActiveOutcome, WindowId};
use crate::{MultiplexerError, error::MultiplexerResult};
use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct SessionId(String);

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SessionState(HashMap<SessionId, Session>);

impl SessionState {
    #[inline]
    pub fn register(&mut self, id: SessionId, session: Session) {
        self.0.insert(id, session);
    }

    #[inline]
    pub fn get(&self, id: &SessionId) -> MultiplexerResult<&Session> {
        self.0
            .get(id)
            .ok_or_else(|| MultiplexerError::SessionNotFound(id.clone()))
    }

    #[inline]
    pub fn get_mut(&mut self, id: &SessionId) -> MultiplexerResult<&mut Session> {
        self.0
            .get_mut(id)
            .ok_or_else(|| MultiplexerError::SessionNotFound(id.clone()))
    }

    #[inline]
    pub fn remove(&mut self, id: &SessionId) -> MultiplexerResult<Session> {
        self.0
            .remove(id)
            .ok_or_else(|| crate::error::MultiplexerError::SessionNotFound(id.clone()))
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&SessionId, &Session)> {
        self.0.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&SessionId, &mut Session)> {
        self.0.iter_mut()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    #[serde(rename = "linkedWindows")]
    pub linked_windows: Vec<WindowId>,
    pub active_window: Option<WindowId>,
}

impl Session {
    /// Construct a session with no windows. `active_window` becomes `Some` the
    /// first time a window is attached.
    pub fn empty(id: SessionId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            linked_windows: Vec::new(),
            active_window: None,
        }
    }

    #[inline]
    pub fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Append `window_id`; promote to `active_window` if none was set.
    pub fn attach_window(&mut self, window_id: WindowId) {
        if self.active_window.is_none() {
            self.active_window = Some(window_id.clone());
        }
        self.linked_windows.push(window_id);
    }

    /// Step `active_window` to the next or previous entry in
    /// `linked_windows` with wrap-around. Returns `Unchanged` for
    /// empty and single-window sessions. Defensively rebases to the
    /// first linked window when `active_window` is `None` (or no
    /// longer in `linked_windows`) despite the list being non-empty —
    /// that case is an invariant violation but the recovery is
    /// silent, mirroring `detach_window`'s own first-window fallback.
    pub fn cycle_active_window(
        &mut self,
        direction: CycleDirection,
    ) -> MultiplexerResult<SetActiveOutcome> {
        let len = self.linked_windows.len();
        if len == 0 {
            return Ok(SetActiveOutcome::Unchanged);
        }
        if len == 1 {
            return Ok(SetActiveOutcome::Unchanged);
        }

        let current_idx = self
            .active_window
            .as_ref()
            .and_then(|w| self.linked_windows.iter().position(|x| x == w));

        let Some(idx) = current_idx else {
            let target = self.linked_windows[0].clone();
            self.active_window = Some(target);
            return Ok(SetActiveOutcome::Changed);
        };

        let new_idx = match direction {
            CycleDirection::Next => (idx + 1) % len,
            CycleDirection::Prev => idx.checked_sub(1).unwrap_or(len - 1),
        };
        let target = self.linked_windows[new_idx].clone();
        self.active_window = Some(target);
        Ok(SetActiveOutcome::Changed)
    }

    /// Remove `window_id` from `linked_windows`; if it was active, fall back to the
    /// first remaining window (or `None` if empty).
    pub fn detach_window(&mut self, window_id: &WindowId) {
        self.linked_windows.retain(|w| w != window_id);
        if self.active_window.as_ref() == Some(window_id) {
            self.active_window = self.linked_windows.first().cloned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_session() -> Session {
        Session::empty(SessionId::new(), "test")
    }

    #[test]
    fn cycle_active_window_empty_session_returns_unchanged() {
        let mut s = fresh_session();
        let outcome = s.cycle_active_window(CycleDirection::Next).unwrap();
        assert!(matches!(outcome, SetActiveOutcome::Unchanged));
        assert!(s.active_window.is_none());
        assert!(s.linked_windows.is_empty());
    }

    #[test]
    fn cycle_active_window_single_window_returns_unchanged() {
        let mut s = fresh_session();
        let only = WindowId::new();
        s.attach_window(only.clone());
        let outcome = s.cycle_active_window(CycleDirection::Next).unwrap();
        assert!(matches!(outcome, SetActiveOutcome::Unchanged));
        assert_eq!(s.active_window, Some(only));
    }

    #[test]
    fn cycle_active_window_next_advances_with_wrap() {
        let mut s = fresh_session();
        let w0 = WindowId::new();
        let w1 = WindowId::new();
        s.attach_window(w0.clone());
        s.attach_window(w1.clone());
        assert_eq!(s.active_window, Some(w0.clone()));

        let outcome = s.cycle_active_window(CycleDirection::Next).unwrap();
        assert!(matches!(outcome, SetActiveOutcome::Changed));
        assert_eq!(s.active_window, Some(w1.clone()));

        let outcome = s.cycle_active_window(CycleDirection::Next).unwrap();
        assert!(matches!(outcome, SetActiveOutcome::Changed));
        assert_eq!(s.active_window, Some(w0));
    }

    #[test]
    fn cycle_active_window_prev_wraps_to_last() {
        let mut s = fresh_session();
        let w0 = WindowId::new();
        let w1 = WindowId::new();
        s.attach_window(w0.clone());
        s.attach_window(w1.clone());
        let outcome = s.cycle_active_window(CycleDirection::Prev).unwrap();
        assert!(matches!(outcome, SetActiveOutcome::Changed));
        assert_eq!(s.active_window, Some(w1));
    }

    #[test]
    fn cycle_active_window_rebases_when_active_missing() {
        let mut s = fresh_session();
        let w0 = WindowId::new();
        let w1 = WindowId::new();
        s.attach_window(w0.clone());
        s.attach_window(w1.clone());
        s.active_window = Some(WindowId::new());

        let outcome = s.cycle_active_window(CycleDirection::Next).unwrap();
        assert!(matches!(outcome, SetActiveOutcome::Changed));
        assert_eq!(
            s.active_window,
            Some(w0),
            "silent rebase must land on linked_windows[0]"
        );
    }

    #[test]
    fn cycle_active_window_single_window_with_stale_active_returns_unchanged() {
        let mut s = fresh_session();
        let only = WindowId::new();
        s.attach_window(only.clone());
        s.active_window = Some(WindowId::new());

        let outcome = s.cycle_active_window(CycleDirection::Next).unwrap();
        assert!(
            matches!(outcome, SetActiveOutcome::Unchanged),
            "single-window session must always be Unchanged, even with stale active"
        );
    }
}
