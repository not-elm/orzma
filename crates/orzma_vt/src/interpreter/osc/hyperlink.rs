//! The hyperlink an `OSC 8` opens and closes.

use crate::hyperlink::HyperlinkUri;

/// What an `OSC 8` asks of the hyperlink the cursor paints with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HyperlinkRequest {
    /// Opens a link on `uri`.
    Open {
        /// The `id=` parameter as written, absent when the key was
        /// omitted. An empty value is kept rather than folded into an
        /// absent one.
        id: Option<String>,
        /// The link target.
        uri: HyperlinkUri,
    },
    /// Stops painting cells as part of any link.
    Close,
}

impl HyperlinkRequest {
    /// The request an `OSC 8` makes; `None` for every other operating
    /// system command and for an `OSC 8` carrying fewer than three
    /// parameters.
    ///
    /// The command arrives split on every `;`, and the pieces after the
    /// parameters are rejoined, so a target keeps its semicolons. An
    /// empty target closes the open link whatever the parameters say.
    /// Bytes that are not valid UTF-8 become the replacement character
    /// rather than emptying the target.
    pub fn parse(params: &[&[u8]]) -> Option<Self> {
        let [b"8", link_params, target @ ..] = params else {
            return None;
        };
        if target.is_empty() {
            return None;
        }
        let uri = String::from_utf8_lossy(&target.join(&b';')).into_owned();
        if uri.is_empty() {
            return Some(Self::Close);
        }
        Some(Self::Open {
            id: Self::id_of(link_params),
            uri: HyperlinkUri::new(uri),
        })
    }

    /// The `id=` parameter's value as written, or `None` when the key is
    /// absent. The parameters are colon-separated `key=value` pairs, and
    /// a key this terminal does not answer is skipped.
    fn id_of(link_params: &[u8]) -> Option<String> {
        link_params
            .split(|byte| *byte == b':')
            .find_map(|pair| pair.strip_prefix(b"id="))
            .map(|value| String::from_utf8_lossy(value).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a hyperlink with an id decodes to an open naming it.
    ///
    /// Case: a build tool tags its error link with `id=err1` so the two
    /// halves of a wrapped path highlight together.
    #[test]
    fn a_hyperlink_with_an_id_decodes_to_an_open() {
        let params: [&[u8]; 3] = [b"8", b"id=err1", b"https://a.example"];
        assert_eq!(
            HyperlinkRequest::parse(&params),
            Some(HyperlinkRequest::Open {
                id: Some("err1".to_owned()),
                uri: HyperlinkUri::new("https://a.example"),
            })
        );
    }

    /// Asserts that an empty uri decodes to a close whatever the
    /// parameters carry.
    ///
    /// Case: a program finishes a link and emits the closing sequence,
    /// having left its `id=` parameter in place.
    #[test]
    fn an_empty_uri_decodes_to_a_close() {
        let bare: [&[u8]; 3] = [b"8", b"", b""];
        let with_params: [&[u8]; 3] = [b"8", b"id=err1", b""];
        assert_eq!(
            HyperlinkRequest::parse(&bare),
            Some(HyperlinkRequest::Close)
        );
        assert_eq!(
            HyperlinkRequest::parse(&with_params),
            Some(HyperlinkRequest::Close)
        );
    }

    /// Asserts that a uri carrying semicolons is rejoined whole.
    ///
    /// Case: a program links to a URL whose path carries matrix
    /// parameters, which need no percent-encoding.
    #[test]
    fn a_uri_carrying_semicolons_is_rejoined() {
        let params: [&[u8]; 5] = [b"8", b"", b"https://a.example/a", b"b", b"c"];
        assert_eq!(
            HyperlinkRequest::parse(&params),
            Some(HyperlinkRequest::Open {
                id: None,
                uri: HyperlinkUri::new("https://a.example/a;b;c"),
            })
        );
    }

    /// Asserts that an unknown parameter key is ignored and the id is
    /// still found beside it.
    ///
    /// Case: a program emits a parameter this terminal does not answer
    /// alongside the id it does.
    #[test]
    fn an_unknown_parameter_key_is_ignored() {
        let params: [&[u8]; 3] = [b"8", b"foo=bar:id=7:baz=quux", b"https://a.example"];
        assert_eq!(
            HyperlinkRequest::parse(&params),
            Some(HyperlinkRequest::Open {
                id: Some("7".to_owned()),
                uri: HyperlinkUri::new("https://a.example"),
            })
        );
    }

    /// Asserts that an empty id is passed through as written rather than
    /// folded into an absent one here.
    ///
    /// Case: a script interpolates an unset shell variable into its
    /// `id=` parameter.
    #[test]
    fn an_empty_id_is_passed_through_as_written() {
        let params: [&[u8]; 3] = [b"8", b"id=", b"https://a.example"];
        assert_eq!(
            HyperlinkRequest::parse(&params),
            Some(HyperlinkRequest::Open {
                id: Some(String::new()),
                uri: HyperlinkUri::new("https://a.example"),
            })
        );
    }

    /// Asserts that a command carrying fewer than three parameters is
    /// refused rather than read as a close.
    ///
    /// Case: a program's output is cut off mid-sequence, so a hyperlink
    /// command arrives without its target field.
    #[test]
    fn a_hyperlink_missing_its_uri_field_is_refused() {
        let bare: [&[u8]; 1] = [b"8"];
        let one_field: [&[u8]; 2] = [b"8", b"id=7"];
        assert_eq!(HyperlinkRequest::parse(&bare), None);
        assert_eq!(HyperlinkRequest::parse(&one_field), None);
    }

    /// Asserts that every operating system command other than the
    /// hyperlink is refused.
    ///
    /// Case: an `OSC 4` palette command reaches the dispatcher, which
    /// offers it to every decoder in turn.
    #[test]
    fn another_operating_system_command_is_refused() {
        let params: [&[u8]; 3] = [b"4", b"1", b"rgb:01/02/03"];
        assert_eq!(HyperlinkRequest::parse(&params), None);
    }

    /// Asserts that a uri whose bytes are not valid UTF-8 is kept with
    /// the replacement character rather than emptied.
    ///
    /// Case: a program writes a raw Latin-1 byte into a link target.
    #[test]
    fn a_uri_with_invalid_utf8_keeps_a_replacement_character() {
        let params: [&[u8]; 3] = [b"8", b"", b"https://a.example/\xff"];
        assert_eq!(
            HyperlinkRequest::parse(&params),
            Some(HyperlinkRequest::Open {
                id: None,
                uri: HyperlinkUri::new("https://a.example/\u{fffd}"),
            })
        );
    }
}
