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
}
