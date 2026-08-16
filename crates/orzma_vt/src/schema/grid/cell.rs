use crate::{
    hyperlink::HyperlinkUri,
    schema::{Color, GridPoint},
};

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

#[cfg(feature = "alacritty")]
impl<'a> SourceHyperlink {
    pub fn from_alacritty_hyperlink(
        link: &'a alacritty_terminal::term::cell::Hyperlink,
    ) -> SourceHyperlink {
        Self {
            id: HyperlinkSourceId(link.id().to_string()),
            uri: HyperlinkUri(link.uri().to_string()),
        }
    }
}

#[cfg(all(test, feature = "alacritty"))]
mod tests {
    use super::*;
    use alacritty_terminal::term::cell::Hyperlink;

    /// Asserts that an explicitly empty `id=` is replaced with a unique identifier.
    ///
    /// Case: a program writes `OSC 8 ; id= ; https://a.example ST` twice
    /// for the same URL. vte passes the empty value through as
    /// `Some("")`, so alacritty skips its own auto-numbering and both
    /// links arrive carrying an empty id. OSC 8 groups cells only by a
    /// *nonempty* id, so the two must stay separate links rather than
    /// merging into one hover group downstream.
    #[test]
    fn empty_explicit_source_id_is_replaced_at_the_backend_boundary() {
        let first = Hyperlink::new(Some(""), "https://a.example".to_owned());
        let second = Hyperlink::new(Some(""), "https://a.example".to_owned());
        let first = SourceHyperlink::from_alacritty_hyperlink(&first);
        let second = SourceHyperlink::from_alacritty_hyperlink(&second);
        assert_ne!(first.id, HyperlinkSourceId::new(String::new()));
        assert_ne!(first.id, second.id);
    }

    /// Asserts that a nonempty explicit `id=` is carried through unchanged.
    ///
    /// Case: an application labels a link `id=42` precisely so its
    /// fragments group together across a wrap. The boundary rewrites only
    /// the empty id; rewriting every id would destroy the grouping the
    /// application asked for.
    #[test]
    fn nonempty_explicit_source_id_is_preserved() {
        let link = Hyperlink::new(Some("42"), "https://a.example".to_owned());
        let source = SourceHyperlink::from_alacritty_hyperlink(&link);
        assert_eq!(source.id, HyperlinkSourceId::new("42".to_owned()));
    }

    /// Asserts that an omitted `id=` keeps alacritty's auto-generated identifier.
    ///
    /// Case: a program prints a plain OSC 8 link with no id parameter, so
    /// alacritty already assigns a unique `N_alacritty` value. That value
    /// is nonempty and unique, so the boundary leaves it alone instead of
    /// substituting an identifier of its own.
    #[test]
    fn omitted_source_id_keeps_the_alacritty_generated_value() {
        let link = Hyperlink::new(None::<&str>, "https://a.example".to_owned());
        let source = SourceHyperlink::from_alacritty_hyperlink(&link);
        assert_eq!(source.id, HyperlinkSourceId::new(link.id().to_owned()));
        assert_ne!(source.id, HyperlinkSourceId::new(String::new()));
    }
}
