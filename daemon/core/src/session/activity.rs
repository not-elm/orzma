use crate::define_string_new_type;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct ActivityStore(HashMap<ActivityId, Activity>);

#[derive(Debug)]
pub struct Activity {
    pub id: ActivityId,
    pub name: String,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            name: "Terminal".to_string(),
            id: ActivityId::new(),
        }
    }
}

define_string_new_type!(ActivityId);
