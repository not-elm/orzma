use crate::error::{MultiplexerError, MultiplexerResult};
use crate::window::pane::activity::{Activity, ActivityId};
use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct PaneId(String);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[must_use]
pub enum SetActiveOutcome {
    Unchanged,
    Changed,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Pane {
    pub id: PaneId,
    pub activities: Vec<Activity>,
    pub active_activity: ActivityId,
}

impl Pane {
    /// Create a Pane that initially contains one Activity. The Activity's id
    /// becomes the active_activity.
    pub fn new(id: PaneId, activity: Activity) -> Self {
        let aid = activity.id.clone();
        Self {
            id,
            activities: vec![activity],
            active_activity: aid,
        }
    }

    /// Add an Activity to this Pane as a new tab. Does NOT change
    /// `active_activity`. Returns `ActivityIdConflict` if the id already exists.
    pub fn add_activity(&mut self, activity: Activity) -> MultiplexerResult<()> {
        if self.has_activity(&activity.id) {
            return Err(MultiplexerError::ActivityIdConflict(activity.id));
        }
        self.activities.push(activity);
        Ok(())
    }

    /// Set `active_activity`. Returns `Unchanged` if already active so the
    /// caller can suppress redundant broadcasts. Returns `ActivityNotInPane`
    /// if the aid is not in this Pane.
    pub fn set_active_activity(&mut self, aid: &ActivityId) -> MultiplexerResult<SetActiveOutcome> {
        if !self.has_activity(aid) {
            return Err(MultiplexerError::ActivityNotInPane {
                pane: self.id.clone(),
                activity: aid.clone(),
            });
        }
        if &self.active_activity == aid {
            return Ok(SetActiveOutcome::Unchanged);
        }
        self.active_activity = aid.clone();
        Ok(SetActiveOutcome::Changed)
    }

    pub fn activity(&self, aid: &ActivityId) -> Option<&Activity> {
        self.activities.iter().find(|a| &a.id == aid)
    }

    pub fn activity_mut(&mut self, aid: &ActivityId) -> Option<&mut Activity> {
        self.activities.iter_mut().find(|a| &a.id == aid)
    }

    pub fn has_activity(&self, aid: &ActivityId) -> bool {
        self.activities.iter().any(|a| &a.id == aid)
    }

    /// Drain the ids of all Activities in this Pane. Used by close cascade
    /// to tell the caller which PTY / registry entries to tear down.
    pub fn activity_ids(&self) -> impl Iterator<Item = &ActivityId> {
        self.activities.iter().map(|a| &a.id)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PaneState(HashMap<PaneId, Pane>);

impl PaneState {
    #[inline]
    pub fn insert(&mut self, pane: Pane) {
        self.0.insert(pane.id.clone(), pane);
    }

    #[inline]
    pub fn remove(&mut self, id: &PaneId) -> MultiplexerResult<Pane> {
        self.0
            .remove(id)
            .ok_or_else(|| MultiplexerError::PaneNotFound(id.clone()))
    }

    #[inline]
    pub fn get(&self, id: &PaneId) -> Option<&Pane> {
        self.0.get(id)
    }

    #[inline]
    pub fn get_mut(&mut self, id: &PaneId) -> Option<&mut Pane> {
        self.0.get_mut(id)
    }

    #[inline]
    pub fn contains_key(&self, id: &PaneId) -> bool {
        self.0.contains_key(id)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&PaneId, &Pane)> {
        self.0.iter()
    }

    #[inline]
    pub fn ids(&self) -> impl Iterator<Item = &PaneId> {
        self.0.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_pane() -> (Pane, ActivityId) {
        let pid = PaneId::new();
        let aid = ActivityId::new();
        let activity = Activity::terminal(aid.clone());
        let pane = Pane::new(pid, activity);
        (pane, aid)
    }

    #[test]
    fn pane_new_initializes_active_activity_to_only_activity() {
        let (pane, aid) = fresh_pane();
        assert_eq!(pane.activities.len(), 1);
        assert_eq!(pane.active_activity, aid);
    }

    #[test]
    fn add_activity_appends_without_changing_active() {
        let (mut pane, original_aid) = fresh_pane();
        let new_aid = ActivityId::new();
        pane.add_activity(Activity::terminal(new_aid.clone()))
            .unwrap();
        assert_eq!(pane.activities.len(), 2);
        assert_eq!(pane.active_activity, original_aid);
    }

    #[test]
    fn add_activity_rejects_duplicate_id() {
        let (mut pane, aid) = fresh_pane();
        let err = pane
            .add_activity(Activity::terminal(aid.clone()))
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::ActivityIdConflict(_)));
    }

    #[test]
    fn set_active_activity_changes_to_new_aid() {
        let (mut pane, original_aid) = fresh_pane();
        let new_aid = ActivityId::new();
        pane.add_activity(Activity::terminal(new_aid.clone()))
            .unwrap();
        let outcome = pane.set_active_activity(&new_aid).unwrap();
        assert!(matches!(outcome, SetActiveOutcome::Changed));
        assert_eq!(pane.active_activity, new_aid);
        let _ = original_aid;
    }

    #[test]
    fn set_active_activity_unchanged_when_already_active() {
        let (mut pane, aid) = fresh_pane();
        let outcome = pane.set_active_activity(&aid).unwrap();
        assert!(matches!(outcome, SetActiveOutcome::Unchanged));
    }

    #[test]
    fn set_active_activity_unknown_returns_not_in_pane() {
        let (mut pane, _) = fresh_pane();
        let phantom = ActivityId::new();
        let err = pane.set_active_activity(&phantom).unwrap_err();
        assert!(matches!(err, MultiplexerError::ActivityNotInPane { .. }));
    }
}
