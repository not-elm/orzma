use crate::define_string_new_type;
use serde::Serialize;

#[derive(Debug, Serialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_serializes_to_id_and_name() {
        let a = Activity::default();
        let v: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert!(v.get("id").and_then(|x| x.as_str()).is_some());
        assert_eq!(v.get("name").and_then(|x| x.as_str()), Some("Terminal"));
    }
}
