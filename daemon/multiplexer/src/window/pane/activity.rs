use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct ActivityId(String);

/// Storage profile assigned to a Browser Activity.
///
/// `Named` profiles are persisted to disk under the ozmux data dir and
/// shared across activities that name the same profile. `Incognito` is an
/// in-memory profile that is discarded when the activity closes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserProfile {
    /// Disk-persistent named profile.
    Named { name: String },
    /// Ephemeral in-memory profile.
    Incognito,
}

impl Default for BrowserProfile {
    fn default() -> Self {
        BrowserProfile::Named {
            name: "default".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ActivityKind {
    Terminal,
    Extension {
        html_root: std::path::PathBuf,
    },
    Browser {
        initial_url: Option<String>,
        #[serde(default)]
        profile: BrowserProfile,
    },
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
    pub fn browser(id: ActivityId, initial_url: Option<String>, profile: BrowserProfile) -> Self {
        Self {
            id,
            name: "Browser".to_string(),
            kind: ActivityKind::Browser {
                initial_url,
                profile,
            },
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
            profile: BrowserProfile::default(),
        };
        let json = serde_json::to_value(&kind).unwrap();
        assert_eq!(json["type"], "browser");
        assert_eq!(json["initial_url"], "https://example.com");
        let back: ActivityKind = serde_json::from_value(json).unwrap();
        assert!(
            matches!(back, ActivityKind::Browser { initial_url: Some(ref u), .. } if u == "https://example.com")
        );
    }

    #[test]
    fn activity_kind_browser_round_trips_without_initial_url() {
        let kind = ActivityKind::Browser {
            initial_url: None,
            profile: BrowserProfile::default(),
        };
        let json = serde_json::to_value(&kind).unwrap();
        assert!(matches!(
            serde_json::from_value::<ActivityKind>(json).unwrap(),
            ActivityKind::Browser {
                initial_url: None,
                ..
            }
        ));
    }

    #[test]
    fn activity_browser_constructor_sets_name() {
        let a = Activity::browser(
            ActivityId::new(),
            Some("https://example.com".into()),
            BrowserProfile::default(),
        );
        assert_eq!(a.name, "Browser");
        assert!(matches!(a.kind, ActivityKind::Browser { .. }));
    }

    #[test]
    fn activity_kind_browser_round_trips_with_named_profile() {
        let kind = ActivityKind::Browser {
            initial_url: Some("https://example.com".into()),
            profile: BrowserProfile::Named {
                name: "work".into(),
            },
        };
        let json = serde_json::to_value(&kind).unwrap();
        assert_eq!(json["type"], "browser");
        assert_eq!(json["profile"]["kind"], "named");
        assert_eq!(json["profile"]["name"], "work");
        let back: ActivityKind = serde_json::from_value(json).unwrap();
        assert!(matches!(
            back,
            ActivityKind::Browser { profile: BrowserProfile::Named { name: ref n }, .. } if n == "work"
        ));
    }

    #[test]
    fn activity_kind_browser_round_trips_incognito() {
        let kind = ActivityKind::Browser {
            initial_url: None,
            profile: BrowserProfile::Incognito,
        };
        let json = serde_json::to_value(&kind).unwrap();
        assert_eq!(json["profile"]["kind"], "incognito");
        assert!(matches!(
            serde_json::from_value::<ActivityKind>(json).unwrap(),
            ActivityKind::Browser {
                profile: BrowserProfile::Incognito,
                ..
            }
        ));
    }

    #[test]
    fn activity_kind_browser_profile_defaults_when_field_absent() {
        let old_json = serde_json::json!({
            "type": "browser",
            "initial_url": null
        });
        let kind: ActivityKind = serde_json::from_value(old_json).unwrap();
        assert!(matches!(
            kind,
            ActivityKind::Browser { profile: BrowserProfile::Named { name: ref n }, .. } if n == "default"
        ));
    }

    #[test]
    fn browser_profile_default_is_named_default() {
        assert!(matches!(
            BrowserProfile::default(),
            BrowserProfile::Named { name: ref n } if n == "default"
        ));
    }
}
