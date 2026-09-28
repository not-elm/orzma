//! Tests for scanning text for URLs.

use super::*;
use crate::hyperlink::is_allowed;

/// The URLs `UrlMatch::scan` finds in `text`, as the text they cover.
fn urls(text: &str) -> Vec<&str> {
    UrlMatch::scan(text)
        .into_iter()
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

/// Asserts that ranges are byte offsets, correct after multibyte text,
/// and that the raw range keeps the trimmed tail.
///
/// Case: a Japanese log line ends a sentence with a URL.
#[test]
fn ranges_are_byte_offsets_and_raw_keeps_the_trimmed_tail() {
    assert_eq!(
        UrlMatch::scan("日本 https://a.example."),
        [UrlMatch {
            url: 7..24,
            raw: 7..25
        }]
    );
}
