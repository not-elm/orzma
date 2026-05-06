use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ActivityKind {
    Terminal,
    Custom { iframe_path: String },
}

#[derive(Debug, Serialize)]
pub struct Activity {
    id: ActivityId,
    name: String,
    kind: ActivityKind,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            id: ActivityId::new(),
            name: "Terminal".to_string(),
            kind: ActivityKind::Terminal,
        }
    }
}

impl Activity {
    pub const fn id(&self) -> &ActivityId {
        &self.id
    }
    pub const fn kind(&self) -> &ActivityKind {
        &self.kind
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct ActivityId(String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_default_has_terminal_kind() {
        let a = Activity::default();
        assert!(matches!(a.kind(), ActivityKind::Terminal));
    }

    #[test]
    fn activity_serializes_to_id_name_and_nested_kind() {
        let a = Activity::default();
        let v: serde_json::Value = serde_json::to_value(&a).unwrap();
        assert!(v.get("id").and_then(|x| x.as_str()).is_some());
        assert_eq!(v.get("name").and_then(|x| x.as_str()), Some("Terminal"));
        // kind is nested: {"kind": {"type": "terminal"}}
        assert_eq!(
            v.get("kind")
                .and_then(|k| k.get("type"))
                .and_then(|t| t.as_str()),
            Some("terminal"),
        );
    }
}
