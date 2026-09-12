//! Pure VT-encoder for paste input: translates clipboard text into the
//! byte sequence the PTY expects, honouring bracketed-paste mode
//! (DECSET 2004).

/// 7-bit bracketed-paste start marker (`ESC [ 200 ~`).
const START_7BIT: &str = "\x1b[200~";
/// 7-bit bracketed-paste end marker (`ESC [ 201 ~`).
const END_7BIT: &str = "\x1b[201~";
/// C1 bracketed-paste start marker (`U+009B 200 ~`).
const START_C1: &str = "\u{9b}200~";
/// C1 bracketed-paste end marker (`U+009B 201 ~`).
const END_C1: &str = "\u{9b}201~";

/// Encodes a paste for the PTY: bracketed framing with embedded-marker
/// stripping when `bracketed`, newline normalization otherwise.
pub(super) fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        encode_bracketed(text)
    } else {
        normalize_newlines(text)
    }
}

/// Strips every embedded occurrence of the four bracketed-paste marker
/// forms — 7-bit `ESC [ 200~` / `ESC [ 201~` and C1 `U+009B 200~` /
/// `U+009B 201~` — then wraps the sanitized body in `ESC [ 200 ~` ...
/// `ESC [ 201 ~`. The body is otherwise passed through byte-for-byte.
///
/// A marker that an earlier removal re-exposes is stripped too
/// (`ESC [ ESC [ 201 ~ 201 ~` leaves nothing), so no marker survives
/// into the body.
fn encode_bracketed(text: &str) -> Vec<u8> {
    let mut body = text.to_owned();
    loop {
        let next = body
            .replace(START_7BIT, "")
            .replace(END_7BIT, "")
            .replace(START_C1, "")
            .replace(END_C1, "");
        if next == body {
            break;
        }
        body = next;
    }
    let mut out = Vec::with_capacity(body.len() + START_7BIT.len() + END_7BIT.len());
    out.extend_from_slice(START_7BIT.as_bytes());
    out.extend_from_slice(body.as_bytes());
    out.extend_from_slice(END_7BIT.as_bytes());
    out
}

/// Normalizes line endings so shells receive one `\r` per line: `\r\n`
/// collapses to `\r`, a lone `\n` becomes `\r`, and existing `\r` bytes
/// pass through. Nothing else is filtered, so control bytes reach the PTY
/// as-is.
fn normalize_newlines(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\r' {
            out.push(b'\r');
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                i += 2;
                continue;
            }
        } else if b == b'\n' {
            out.push(b'\r');
        } else {
            out.push(b);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four marker spellings the encoder must strip from a bracketed body.
    const ALL_MARKERS: [&str; 4] = [START_7BIT, END_7BIT, START_C1, END_C1];

    /// Wraps `body` in the 7-bit frame the encoder is expected to emit.
    fn wrapped(body: &str) -> Vec<u8> {
        let mut out = START_7BIT.as_bytes().to_vec();
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(END_7BIT.as_bytes());
        out
    }

    /// Strips the outer frame from a bracketed encoding and returns the
    /// body, panicking when the output is not framed at all.
    fn body_of(encoded: &[u8]) -> String {
        let body = encoded
            .strip_prefix(START_7BIT.as_bytes())
            .expect("bracketed output must start with ESC[200~");
        let body = body
            .strip_suffix(END_7BIT.as_bytes())
            .expect("bracketed output must end with ESC[201~");
        String::from_utf8(body.to_vec()).expect("body must stay valid UTF-8")
    }

    /// Adversarial inputs shared by the invariant sweep: every marker
    /// form embedded and doubled, adjacency, a cross-form mix, and the
    /// nests where removing one marker re-exposes another.
    fn adversarial_inputs() -> Vec<String> {
        let mut inputs: Vec<String> = vec![
            "\x1b[\x1b[201~201~".into(),
            "\x1b[\x1b[200~200~".into(),
            "\u{9b}\x1b[201~201~".into(),
            "\x1b[\x1b[\x1b[201~201~201~".into(),
            "a\x1b[201~b\u{9b}200~c".into(),
            "\x1b[200~\x1b[201~x".into(),
        ];
        for marker in ALL_MARKERS {
            inputs.push(format!("foo{marker}bar"));
            inputs.push(format!("{marker}{marker}"));
        }
        inputs
    }

    /// Asserts that clean text is framed by the 7-bit start/end markers.
    ///
    /// Case: the user pastes plain text into an app that enabled
    /// DECSET 2004.
    #[test]
    fn bracketed_wraps_clean_body() {
        assert_eq!(encode_paste("hello", true), b"\x1b[200~hello\x1b[201~");
    }

    /// Asserts that each of the four marker spellings is stripped from
    /// the body individually.
    ///
    /// Case: hostile clipboard content embeds one marker to terminate the
    /// paste frame early and have the remainder run as typed input.
    #[test]
    fn bracketed_strips_each_marker_form() {
        for marker in ALL_MARKERS {
            assert_eq!(
                encode_paste(&format!("foo{marker}bar"), true),
                wrapped("foobar"),
                "marker {marker:?} must be stripped from the body"
            );
        }
    }

    /// Asserts that repeated and adjacent markers are all removed.
    ///
    /// Case: hostile clipboard content repeats a marker or butts two
    /// markers together.
    #[test]
    fn bracketed_strips_repeated_and_adjacent_markers() {
        assert_eq!(
            encode_paste("a\x1b[201~b\x1b[201~c", true),
            wrapped("abc"),
            "every repeated occurrence must be stripped"
        );
        assert_eq!(
            encode_paste("\x1b[200~\x1b[201~x", true),
            wrapped("x"),
            "adjacent start+end markers must both be stripped"
        );
    }

    /// Asserts that 7-bit and C1 spellings mixed in one body are both
    /// stripped.
    ///
    /// Case: hostile clipboard content alternates the 7-bit and C1 marker
    /// forms within one body.
    #[test]
    fn bracketed_strips_mixed_marker_forms() {
        assert_eq!(encode_paste("a\x1b[201~b\u{9b}200~c", true), wrapped("abc"));
    }

    /// Asserts that markers re-exposed by an earlier removal are also
    /// stripped, for start, end, and cross-form (7-bit removal exposing
    /// a C1 marker) nests.
    ///
    /// Case: hostile clipboard content nests one marker inside another,
    /// as in `ESC [ ESC [ 201 ~ 201 ~`, so removing the inner marker
    /// assembles a fresh one from the leftovers.
    #[test]
    fn bracketed_fixed_point_strips_reexposed_markers() {
        for input in [
            "\x1b[\x1b[201~201~",
            "\x1b[\x1b[200~200~",
            "\u{9b}\x1b[201~201~",
        ] {
            assert_eq!(
                encode_paste(input, true),
                wrapped(""),
                "re-exposed marker must be stripped for input {input:?}"
            );
        }
    }

    /// Asserts that sequences resembling — but not equal to — the
    /// markers survive into the body.
    ///
    /// Case: the user pastes legitimate content that holds another CSI
    /// sequence (`ESC[202~`), a truncated marker, or the bare digits
    /// `200~`.
    #[test]
    fn bracketed_preserves_similar_sequences() {
        for body in ["\x1b[202~", "\x1b[200", "200~", "\x1b]200~"] {
            assert_eq!(
                encode_paste(body, true),
                wrapped(body),
                "non-marker sequence {body:?} must be preserved"
            );
        }
    }

    /// Asserts that empty text still yields a complete, well-formed
    /// frame.
    ///
    /// Case: a caller hands the encoder an empty clipboard read directly
    /// while bracketed-paste mode is on.
    #[test]
    fn bracketed_empty_text_emits_well_formed_brackets() {
        assert_eq!(encode_paste("", true), b"\x1b[200~\x1b[201~");
    }

    /// Asserts that newlines and multi-byte text cross the bracketed
    /// path byte-for-byte.
    ///
    /// Case: the user pastes a multi-line, non-ASCII snippet into an app
    /// that enabled DECSET 2004 and applies its own newline handling.
    #[test]
    fn bracketed_newlines_and_multibyte_pass_through() {
        assert_eq!(encode_paste("a\r\nb\nc\rd", true), wrapped("a\r\nb\nc\rd"));
        assert_eq!(encode_paste("日本語🦀", true), wrapped("日本語🦀"));
    }

    /// Asserts, over the whole adversarial fixture set, that no marker
    /// spelling ever survives into the emitted body.
    ///
    /// Case: each adversarial payload is pasted in turn into an app that
    /// enabled DECSET 2004.
    #[test]
    fn bracketed_body_never_contains_any_marker() {
        for input in adversarial_inputs() {
            let body = body_of(&encode_paste(&input, true));
            for marker in ALL_MARKERS {
                assert!(
                    !body.contains(marker),
                    "marker {marker:?} survived in body {body:?} for input {input:?}"
                );
            }
        }
    }

    /// Asserts that a large marker-only payload (16k markers, ~96 KiB)
    /// terminates and strips to an empty body.
    ///
    /// Case: the user pastes a clipboard payload of about 96 KiB made of
    /// nothing but end markers.
    #[test]
    fn bracketed_large_adversarial_input_terminates() {
        let input = END_7BIT.repeat(16_000);
        assert_eq!(body_of(&encode_paste(&input, true)), "");
    }

    /// Asserts the unbracketed newline rules: `\r\n` collapses to `\r`,
    /// a lone `\n` becomes `\r`, existing `\r` passes through, and a
    /// `\r\n\n` run yields exactly two `\r`.
    ///
    /// Case: the user pastes a multi-line snippet from an editor into a
    /// plain shell, whose line discipline expects one `\r` per line.
    #[test]
    fn unbracketed_normalizes_newlines() {
        for (input, expected) in [
            ("a\r\nb", &b"a\rb"[..]),
            ("a\nb", b"a\rb"),
            ("a\rb", b"a\rb"),
            ("a\r\n\nb", b"a\r\rb"),
        ] {
            assert_eq!(encode_paste(input, false), expected, "input {input:?}");
        }
    }

    /// Asserts that paste markers are NOT stripped on the unbracketed
    /// path while newlines in the same input still normalize.
    ///
    /// Case: the user pastes text that holds a paste marker and a newline
    /// into a shell that left DECSET 2004 off.
    #[test]
    fn unbracketed_does_not_strip_paste_markers() {
        assert_eq!(
            encode_paste("x\x1b[201~\ny", false),
            b"x\x1b[201~\ry".to_vec()
        );
    }

    /// Asserts that empty text encodes to zero bytes.
    ///
    /// Case: a caller hands the encoder an empty clipboard read directly
    /// while bracketed-paste mode is off.
    #[test]
    fn unbracketed_empty_text_emits_nothing() {
        assert_eq!(encode_paste("", false), b"");
    }

    /// Asserts that control characters pass through the unbracketed path
    /// unfiltered: TAB, BEL, ESC, ETX, and NUL.
    ///
    /// Case: the user pastes snippets holding a tab, a bell, or an escape
    /// byte into a shell that left DECSET 2004 off.
    #[test]
    fn unbracketed_control_chars_pass_through() {
        for input in ["a\tb", "a\x07b", "a\x1bb", "a\x03b", "a\0b"] {
            assert_eq!(
                encode_paste(input, false),
                input.as_bytes().to_vec(),
                "control byte in {input:?} must pass through"
            );
        }
    }
}
