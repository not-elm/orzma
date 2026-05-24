use serde::{Deserialize, Serialize};

/// OSC 8 hyperlink: server-assigned wire id → URI mapping.
///
/// Wire id is a monotonic u32 assigned by `crate::vt::hyperlink::HyperlinkInterner`
/// keyed by `(alacritty_id, uri)`. Cells reference these via [`Run::hyperlink_id`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hyperlink {
    /// Monotonic u32 wire id assigned server-side.
    pub id: HyperlinkId,
    /// The hyperlink target URI.
    pub uri: HyperlinkUri,
}
/// Wire-level monotonic hyperlink id.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HyperlinkId(pub u32);

impl HyperlinkId {
    /// Returns the underlying `u32`.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// OSC 8 hyperlink target URI.  
#[derive(Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HyperlinkUri(String);

impl HyperlinkUri {
    /// Wraps a string as a hyperlink URI.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
