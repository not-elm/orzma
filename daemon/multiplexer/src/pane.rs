use crate::{
    activity::ActivityId,
    error::{SessionError, SessionResult},
};
use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct PaneId(String);

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PaneState(HashMap<PaneId, Pane>);

impl PaneState {
    #[inline]
    pub fn insert(&mut self, id: PaneId, pane: Pane) {
        self.0.insert(id, pane);
    }

    #[inline]
    pub fn remove(&mut self, id: &PaneId) -> SessionResult<Pane> {
        self.0
            .remove(id)
            .ok_or_else(|| SessionError::PaneNotFound(id.clone()))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn any_pane_id(&self) -> Option<PaneId> {
        self.0.keys().next().cloned()
    }

    #[inline]
    pub fn get(&self, id: &PaneId) -> Option<&Pane> {
        self.0.get(id)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Pane {
    pub activities: Vec<ActivityId>,
    pub active_activity: ActivityId,
}

impl Pane {
    pub fn new(activity: ActivityId) -> Self {
        Self {
            activities: vec![activity.clone()],
            active_activity: activity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_new_initializes_active_activity_to_only_activity() {
        let aid = ActivityId::new();
        let pane = Pane::new(aid.clone());
        assert_eq!(pane.activities, vec![aid.clone()]);
        assert_eq!(pane.active_activity, aid);
    }
}
