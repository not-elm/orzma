//! Plain-text URL detection: the scanner over text, and the lookup that
//! maps a match back to the viewport cells showing it.

use std::ops::Range;
use unicode_width::UnicodeWidthChar;

/// One URL found in plain text, as byte ranges into that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlMatch {
    /// The URL, with trailing punctuation trimmed.
    pub url: Range<usize>,
    /// The span the scan covered before trimming. It starts where `url`
    /// starts and ends at or after `url`'s end.
    pub raw: Range<usize>,
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
    /// resumes at each match's `raw` end, so a scheme inside a URL never
    /// starts a second match.
    pub fn scan(text: &str) -> Vec<Self> {
        let bytes = text.as_bytes();
        let mut found = Vec::new();
        let mut from = 0;
        while let Some((start, body)) = next_scheme(bytes, from) {
            let (end, voided) = body_end(text, body);
            let url_end = trimmed_end(bytes, body, end);
            if !voided && url_end > body {
                found.push(Self {
                    url: start..url_end,
                    raw: start..end,
                });
            }
            from = end;
        }
        found
    }
}

/// Whether `c` may appear in a URL body.
fn is_url_body(c: char) -> bool {
    c.is_ascii_graphic() && !EXCLUDED_ASCII.contains(&c)
}

/// The byte offset where the next scheme at or after `from` starts, and
/// the offset where its body starts.
fn next_scheme(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    (from..bytes.len()).find_map(|start| {
        let bounded = start
            .checked_sub(1)
            .and_then(|before| bytes.get(before))
            .is_none_or(|byte| !byte.is_ascii_alphanumeric());
        let scheme = SCHEMES.iter().find(|scheme| {
            bytes
                .get(start..start + scheme.len())
                .is_some_and(|window| window.eq_ignore_ascii_case(scheme))
        })?;
        bounded.then_some((start, start + scheme.len()))
    })
}

/// The byte offset where the URL body starting at `body` ends, and
/// whether the character found there voids the URL.
fn body_end(text: &str, body: usize) -> (usize, bool) {
    let Some(rest) = text.get(body..) else {
        return (body, false);
    };
    let mut open: Vec<char> = Vec::new();
    for (offset, c) in rest.char_indices() {
        let at = body + offset;
        match c {
            '(' | '[' => open.push(c),
            ')' | ']' => {
                let opener = if c == ')' { '(' } else { '[' };
                if open.last() != Some(&opener) {
                    return (at, false);
                }
                open.pop();
            }
            _ if is_url_body(c) => {}
            _ => return (at, voids_url(c)),
        }
    }
    (text.len(), false)
}

/// Whether `c`, right after a URL body, voids the URL rather than
/// ending it there.
fn voids_url(c: char) -> bool {
    !c.is_ascii()
        && !c.is_whitespace()
        && !c.is_control()
        && UnicodeWidthChar::width(c) != Some(2)
        && !CLOSING_PUNCTUATION.contains(&c)
}

/// `end` moved back over trailing punctuation, never before `body`.
fn trimmed_end(bytes: &[u8], body: usize, end: usize) -> usize {
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

/// The scheme prefixes a URL may start with.
const SCHEMES: [&[u8]; 4] = [b"http://", b"https://", b"ftp://", b"mailto:"];

/// The printable ASCII characters a URL body never holds.
const EXCLUDED_ASCII: [char; 9] = ['<', '>', '"', '`', '{', '}', '|', '\\', '^'];

/// The non-ASCII characters besides whitespace and width-2 characters
/// that end a URL instead of voiding it.
const CLOSING_PUNCTUATION: [char; 6] = [
    '\u{2019}', '\u{201D}', '\u{00BB}', '\u{203A}', '\u{2013}', '\u{2014}',
];

/// The characters trimmed from the end of a URL.
const TRAILING_TRIM: [u8; 9] = [b'.', b',', b':', b';', b'!', b'?', b'\'', b'(', b'['];

#[cfg(test)]
mod tests;
