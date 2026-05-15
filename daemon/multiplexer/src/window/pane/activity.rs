use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct ActivityId(String);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ActivityKind {
    Terminal,
    Extension { html_root: std::path::PathBuf },
    Browser { initial_url: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub id: ActivityId,
    pub name: String,
    pub kind: ActivityKind,
}

impl Activity {
    /// Construct a Terminal-kind Activity with the given id and a default name.
    pub fn terminal(id: ActivityId) -> Self {
        Self {
            id,
            name: "Terminal".to_string(),
            kind: ActivityKind::Terminal,
        }
    }

    /// Construct an Extension-kind Activity.
    pub fn extension(
        id: ActivityId,
        name: impl Into<String>,
        html_root: std::path::PathBuf,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            kind: ActivityKind::Extension { html_root },
        }
    }

    /// Construct a Browser-kind Activity.
    pub fn browser(id: ActivityId, initial_url: Option<String>) -> Self {
        Self {
            id,
            name: "Browser".to_string(),
            kind: ActivityKind::Browser { initial_url },
        }
    }
}

#[cfg(test)]
mod tests_browser_variant {
    use super::*;

    #[test]
    fn activity_kind_browser_round_trips_with_initial_url() {
        let kind = ActivityKind::Browser {
            initial_url: Some("https://example.com".into()),
        };
        let json = serde_json::to_value(&kind).unwrap();
        assert_eq!(json["type"], "browser");
        assert_eq!(json["initial_url"], "https://example.com");
        let back: ActivityKind = serde_json::from_value(json).unwrap();
        assert!(
            matches!(back, ActivityKind::Browser { initial_url: Some(ref u) } if u == "https://example.com")
        );
    }

    #[test]
    fn activity_kind_browser_round_trips_without_initial_url() {
        let kind = ActivityKind::Browser { initial_url: None };
        let json = serde_json::to_value(&kind).unwrap();
        assert!(matches!(
            serde_json::from_value::<ActivityKind>(json).unwrap(),
            ActivityKind::Browser { initial_url: None }
        ));
    }

    #[test]
    fn activity_browser_constructor_sets_name() {
        let a = Activity::browser(ActivityId::new(), Some("https://example.com".into()));
        assert_eq!(a.name, "Browser");
        assert!(matches!(a.kind, ActivityKind::Browser { .. }));
    }
}
