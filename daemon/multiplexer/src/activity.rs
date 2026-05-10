use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct ActivityState(HashMap<ActivityId, Activity>);

impl ActivityState {
    #[inline]
    pub fn insert(&mut self, id: ActivityId, activity: Activity) {
        self.0.insert(id, activity);
    }

    #[inline]
    pub fn remove(&mut self, id: &ActivityId) {
        self.0.remove(id);
    }

    #[inline]
    pub fn contains(&self, id: &ActivityId) -> bool {
        self.0.contains_key(id)
    }

    #[inline]
    pub fn get(&self, id: &ActivityId) -> Option<&Activity> {
        self.0.get(id)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct ActivityId(String);

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ActivityKind {
    Terminal,
    Extension { iframe_path: String },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Activity {
    pub name: String,
    pub kind: ActivityKind,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            name: "Terminal".to_string(),
            kind: ActivityKind::Terminal,
        }
    }
}
