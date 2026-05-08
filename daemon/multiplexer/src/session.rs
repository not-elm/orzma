use crate::WindowId;
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

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    pub windows: Vec<WindowId>,
    pub active_window: WindowId,
}

impl Session {
    /// Construct a session with no windows. Use `WindowService::create` to add
    /// windows; an empty windows list violates an invariant — `bootstrap_default`
    /// must be paired with a window insert.
    pub fn new(name: impl Into<String>, default_window: WindowId) -> Self {
        Self {
            name: name.into(),
            windows: vec![default_window.clone()],
            active_window: default_window,
        }
    }

    pub fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
}
