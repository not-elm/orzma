use crate::hyperlink::HyperlinkUri;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HyperlinkSourceId(String);

impl HyperlinkSourceId {
    /// Wraps a backend-supplied OSC 8 identifier.
    pub fn new(id: String) -> Self {
        Self(id)
    }
}

#[derive(Debug, Eq, PartialEq, Hash)]
pub struct SourceHyperlink {
    pub id: HyperlinkSourceId,
    pub uri: HyperlinkUri,
}
