use crate::schema::{Color, GridPoint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HyperlinkSourceId(String);

#[derive(Debug, PartialEq)]
pub struct SourceHyperlink {
    pub id: HyperlinkSourceId,
    pub uri: String,
}

#[cfg(feature = "alacritty")]
impl<'a> SourceHyperlink {
    pub fn from_alacritty_hyperlink(
        link: &'a alacritty_terminal::term::cell::Hyperlink,
    ) -> SourceHyperlink {
        Self {
            id: HyperlinkSourceId(link.id().to_string()),
            uri: link.uri().to_string(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct SourceCell {
    pub point: GridPoint,
    pub fg: Color,
    pub bg: Color,
    pub hyperlink: Option<SourceHyperlink>,
}
