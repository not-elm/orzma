use crate::window::WindowId;
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

    /// Remove `window_id` from `linked_windows`; if it was active, fall back to the
    /// first remaining window (or `None` if empty).
    pub fn detach_window(&mut self, window_id: &WindowId) {
        self.linked_windows.retain(|w| w != window_id);
        if self.active_window.as_ref() == Some(window_id) {
            self.active_window = self.linked_windows.first().cloned();
        }
    }
}
