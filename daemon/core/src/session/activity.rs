use crate::define_string_new_type;

#[derive(Debug)]
pub struct Activity {
    id: ActivityId,
    name: String,
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
