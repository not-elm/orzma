use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::session::pane::PaneStore;

mod pane;

#[derive(Clone)]
pub struct SessionStore {
    pub sessions: HashMap<SessionId, Session>,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub pane: PaneStore,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Create the new session-id with a unique identifier
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl AsRef<str> for SessionId {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}
