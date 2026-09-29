//! Plain-text URL scanning: the URLs a string holds, as byte ranges into
//! it.

use crate::screen::cell::GlyphClass;
use std::iter;
use std::ops::Range;

/// One URL found in plain text, as byte ranges into that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UrlMatch {
    /// The URL, with trailing punctuation trimmed.
    pub url: Range<usize>,
    /// Where the scan ended before trimming, at or after `url`'s end.
    pub scan_end: usize,
}

impl UrlMatch {
    /// Every URL in `text`, leftmost first and never overlapping.
    ///
    /// A URL starts with `http://`, `https://`, `ftp://` or `mailto:`,
    /// matched ASCII case-insensitively, at the start of `text` or after a
    /// byte that is not an ASCII letter or digit. Its body runs over
    /// printable ASCII other than ``<>"`{}|\^``, keeping balanced `()` and
    /// `[]`, and ends before an unmatched `)` or `]`; trailing
    /// `.,:;!?'([` are then trimmed. A non-ASCII character right after the
    /// body ends the URL there when it is whitespace, a control, a width-2
    /// character, or one of `’”»›–—`, and voids the URL otherwise. A URL
    /// with nothing left after its scheme is not reported. Scanning
    /// resumes at each match's `scan_end`, so a scheme inside a URL never
    /// starts a second match.
    pub fn scan(text: &str) -> impl Iterator<Item = Self> {
        let bytes = text.as_bytes();
        let mut from = 0;
        iter::from_fn(move || {
            while let Some((start, body)) = next_scheme(bytes, from) {
                match BodyEnd::find(text, body) {
                    BodyEnd::Voided(end) => from = end,
                    BodyEnd::Ends(end) => {
                        from = end;
                        let url_end = trimmed_end(bytes, body, end);
                        if url_end > body {
                            return Some(Self {
                                url: start..url_end,
                                scan_end: end,
                            });
                        }
                    }
                }
            }
            None
        })
    }
}

/// Whether `c` may appear in a URL body.
pub(super) fn is_url_body(c: char) -> bool {
    /// The printable ASCII characters a URL body never holds.
    const EXCLUDED_ASCII: [char; 9] = ['<', '>', '"', '`', '{', '}', '|', '\\', '^'];

    c.is_ascii_graphic() && !EXCLUDED_ASCII.contains(&c)
}

/// The byte offset where the next scheme at or after `from` starts, and
/// the offset where its body starts.
fn next_scheme(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    /// The scheme prefixes a URL may start with.
    const SCHEMES: [&[u8]; 4] = [b"http://", b"https://", b"ftp://", b"mailto:"];

    (from..bytes.len()).find_map(|start| {
        let scheme = SCHEMES.iter().find(|scheme| {
            bytes
                .get(start..start + scheme.len())
                .is_some_and(|window| window.eq_ignore_ascii_case(scheme))
        })?;
        is_word_start(bytes, start).then_some((start, start + scheme.len()))
    })
}

/// Whether `start` is the start of `bytes` or follows a byte that is not
/// an ASCII letter or digit.
fn is_word_start(bytes: &[u8], start: usize) -> bool {
    start
        .checked_sub(1)
        .and_then(|before| bytes.get(before))
        .is_none_or(|byte| !byte.is_ascii_alphanumeric())
}

/// Where a URL body ends, as a byte offset into the scanned text.
enum BodyEnd {
    /// The body ends before this offset.
    Ends(usize),
    /// The character at this offset voids the URL.
    Voided(usize),
}

impl BodyEnd {
    /// The end of the URL body that starts at byte offset `body` in `text`.
    fn find(text: &str, body: usize) -> Self {
        let Some(rest) = text.get(body..) else {
            return Self::Ends(body);
        };
        let mut open: Vec<char> = Vec::new();
        for (offset, c) in rest.char_indices() {
            let at = body + offset;
            match c {
                '(' | '[' => open.push(c),
                ')' | ']' => {
                    let opener = if c == ')' { '(' } else { '[' };
                    if open.last() != Some(&opener) {
                        return Self::Ends(at);
                    }
                    open.pop();
                }
                _ if is_url_body(c) => {}
                _ if voids_url(c) => return Self::Voided(at),
                _ => return Self::Ends(at),
            }
        }
        Self::Ends(text.len())
    }
}

/// Whether `c`, right after a URL body, voids the URL rather than
/// ending it there.
fn voids_url(c: char) -> bool {
    /// The non-ASCII characters besides whitespace and width-2 characters
    /// that end a URL instead of voiding it.
    const CLOSING_PUNCTUATION: [char; 6] = [
        '\u{2019}', '\u{201D}', '\u{00BB}', '\u{203A}', '\u{2013}', '\u{2014}',
    ];

    !c.is_ascii()
        && !c.is_whitespace()
        && !c.is_control()
        && GlyphClass::of(c) != Some(GlyphClass::Wide)
        && !CLOSING_PUNCTUATION.contains(&c)
}

/// `end` moved back over trailing punctuation, never before `body`.
fn trimmed_end(bytes: &[u8], body: usize, end: usize) -> usize {
    /// The characters trimmed from the end of a URL.
    const TRAILING_TRIM: [u8; 9] = [b'.', b',', b':', b';', b'!', b'?', b'\'', b'(', b'['];

    let mut end = end;
    while end > body
        && bytes
            .get(end - 1)
            .is_some_and(|byte| TRAILING_TRIM.contains(byte))
    {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyperlink::is_allowed;

    /// The URLs `UrlMatch::scan` finds in `text`, as the text they cover.
    fn urls(text: &str) -> Vec<&str> {
        UrlMatch::scan(text)
            .filter_map(|found| text.get(found.url))
            .collect()
    }

    /// Asserts that each of the four schemes starts a URL and that every
    /// detected URL passes the opener's allowlist.
    ///
    /// Case: a log prints a web page, a mirror, and a contact address.
    #[test]
    fn every_scheme_starts_a_url_the_opener_allows() {
        let text = "http://a.example https://b.example ftp://c.example mailto:d@e.example";
        let found = urls(text);
        assert_eq!(
            found,
            [
                "http://a.example",
                "https://b.example",
                "ftp://c.example",
                "mailto:d@e.example"
            ]
        );
        assert!(found.iter().all(|url| is_allowed(url)));
    }

    /// Asserts that schemes match without regard to ASCII case.
    ///
    /// Case: a tool prints an upper-case URL and a capitalized mail link.
    #[test]
    fn schemes_match_case_insensitively() {
        assert_eq!(
            urls("HTTPS://A.EXAMPLE Mailto:x@y.example"),
            ["HTTPS://A.EXAMPLE", "Mailto:x@y.example"]
        );
    }

    /// Asserts that a scheme directly after an ASCII letter or digit starts
    /// no URL, while one after any other character does.
    ///
    /// Case: output mentions an `sftp://` host and a pip requirement written
    /// as `git+https://…`.
    #[test]
    fn a_scheme_after_a_letter_or_digit_starts_no_url() {
        assert!(urls("sftp://host xhttp://a 1https://b").is_empty());
        assert_eq!(
            urls("git+https://a.example/r.git (https://b.example"),
            ["https://a.example/r.git", "https://b.example"]
        );
    }

    /// Asserts that whitespace, a control and each excluded ASCII character
    /// end the body.
    ///
    /// Case: a URL is quoted, bracketed, or followed by a tab in program
    /// output.
    #[test]
    fn whitespace_controls_and_excluded_ascii_end_the_body() {
        for text in [
            "<https://a.example>",
            "\"https://a.example\"",
            "https://a.example`x",
            "https://a.example{x",
            "https://a.example}x",
            "https://a.example|x",
            "https://a.example\\x",
            "https://a.example^x",
            "https://a.example\tx",
            "https://a.example x",
        ] {
            assert_eq!(urls(text), ["https://a.example"], "in {text:?}");
        }
    }

    /// Asserts that a width-2 character, Unicode whitespace, a closing quote
    /// or a dash after the body ends the URL there.
    ///
    /// Case: a Japanese sentence or an English quotation runs straight into
    /// a URL without a space.
    #[test]
    fn wide_characters_and_closing_punctuation_end_the_url() {
        assert_eq!(
            urls("https://example.com/docsを参照"),
            ["https://example.com/docs"]
        );
        assert_eq!(
            urls("https://example.com/docs。"),
            ["https://example.com/docs"]
        );
        assert_eq!(urls("\u{201C}https://a.com\u{201D}"), ["https://a.com"]);
        assert_eq!(urls("https://a.com\u{2019}s"), ["https://a.com"]);
        assert_eq!(urls("https://a.com\u{2014}see"), ["https://a.com"]);
        assert_eq!(urls("https://a.com\u{2013}x"), ["https://a.com"]);
        assert_eq!(urls("\u{00AB}https://a.com\u{00BB}"), ["https://a.com"]);
        assert_eq!(urls("https://a.com\u{203A}"), ["https://a.com"]);
        assert_eq!(urls("https://a.com\u{00A0}x"), ["https://a.com"]);
        assert_eq!(urls("https://a.com\u{3000}x"), ["https://a.com"]);
    }

    /// Asserts that any other non-ASCII character after the body voids the
    /// whole URL, and scanning goes on after it.
    ///
    /// Case: a table truncates a pull request URL with an ellipsis, and a
    /// French page title is printed decoded.
    #[test]
    fn other_non_ascii_characters_void_the_url() {
        assert_eq!(
            urls("https://github.com/org/repo/pull/12\u{2026} https://ok.example"),
            ["https://ok.example"]
        );
        assert!(urls("https://fr.wikipedia.org/wiki/Caf\u{00E9}").is_empty());
        assert!(urls("https://a.com/e\u{0301}").is_empty());
    }

    /// Asserts that trailing sentence punctuation and a trailing opening
    /// bracket are trimmed.
    ///
    /// Case: a commit message ends a sentence with a URL, and a script
    /// prints a URL in single quotes.
    #[test]
    fn trailing_punctuation_is_trimmed() {
        assert_eq!(urls("see https://a.com/x."), ["https://a.com/x"]);
        assert_eq!(urls("https://a.com/x, then"), ["https://a.com/x"]);
        assert_eq!(urls("https://a.com/x!?"), ["https://a.com/x"]);
        assert_eq!(urls("'https://a.com/x'"), ["https://a.com/x"]);
        assert_eq!(urls("https://a.com/f("), ["https://a.com/f"]);
        assert_eq!(urls("https://a.com:"), ["https://a.com"]);
    }

    /// Asserts that balanced brackets stay in the URL and an unmatched
    /// closing bracket ends it, even mid-text.
    ///
    /// Case: a commit message puts a Wikipedia link inside parentheses, and
    /// a README holds a Markdown link.
    #[test]
    fn brackets_are_balanced() {
        assert_eq!(
            urls("(see https://en.wikipedia.org/wiki/Foo_(bar))."),
            ["https://en.wikipedia.org/wiki/Foo_(bar)"]
        );
        assert_eq!(urls("[x](https://a.com)"), ["https://a.com"]);
        assert_eq!(urls("https://a.com/x)y"), ["https://a.com/x"]);
        assert_eq!(urls("https://a.com/[1]"), ["https://a.com/[1]"]);
    }

    /// Asserts that a scheme with nothing left after it is not a URL.
    ///
    /// Case: documentation prints bare scheme names.
    #[test]
    fn a_scheme_with_nothing_after_it_is_not_a_url() {
        assert!(urls("https:// mailto: https://. ftp://,").is_empty());
    }

    /// Asserts that a scheme inside a URL starts no second match.
    ///
    /// Case: a Wayback Machine URL embeds the address it archived.
    #[test]
    fn a_scheme_inside_a_url_starts_no_second_match() {
        let text = "https://web.archive.org/web/2020/https://example.com/";
        assert_eq!(urls(text), [text]);
    }

    /// Asserts that offsets are byte offsets, correct after multibyte text,
    /// and that the scan end keeps the trimmed tail.
    ///
    /// Case: a Japanese log line ends a sentence with a URL.
    #[test]
    fn offsets_are_bytes_and_the_scan_end_keeps_the_trimmed_tail() {
        assert_eq!(
            UrlMatch::scan("日本 https://a.example.").collect::<Vec<_>>(),
            [UrlMatch {
                url: 7..24,
                scan_end: 25
            }]
        );
    }
}
