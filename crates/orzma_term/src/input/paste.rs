//! Pure VT-encoder for paste input. Translates clipboard text into the
//! byte sequence the PTY expects, honouring bracketed-paste mode
//! (DECSET 2004). No I/O, no Bevy types — kept pure so unit tests can
//! cover every branch without an `App`.

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
/// `U+009B 201~` — in a fixed-point loop, then wraps the sanitized body
/// in `ESC [ 200 ~` ... `ESC [ 201 ~`. The body is otherwise passed
/// through byte-for-byte.
///
/// The loop is required because removing one marker can re-expose
/// another (`ESC [ ESC [ 201 ~ 201 ~` → `ESC [ 201 ~`). It terminates
/// because every iteration that continues removes at least one marker
/// (six chars or more), bounding the iteration count by
/// `text.len() / 6 + 1`. Closes the paste-injection class documented in
/// kitty commit 668f6fa and Alacritty issue #800.
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
/// pass through. Nothing else is filtered — an unbracketed paste has the
/// same authority as typed input, so control bytes reach the PTY as-is.
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
    /// re-exposure nests the fixed-point loop exists for.
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
    /// Case: the ordinary paste — an app enabled DECSET 2004 and expects
    /// the body delivered between `ESC[200~` and `ESC[201~`.
    #[test]
    fn bracketed_wraps_clean_body() {
        assert_eq!(encode_paste("hello", true), b"\x1b[200~hello\x1b[201~");
    }

    /// Asserts that each of the four marker spellings is stripped from
    /// the body individually.
    ///
    /// Case: hostile clipboard content embedding one marker to terminate
    /// the paste frame early and have the remainder run as typed input —
    /// the kitty 668f6fa / Alacritty #800 injection class. All four
    /// spellings (7-bit and C1, start and end) must be covered because
    /// any single surviving form re-opens the hole.
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
    /// Case: payloads that repeat a marker or butt two markers together,
    /// probing a stripper that only removes the first occurrence or
    /// mis-steps over adjacent matches.
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
    /// Case: a payload alternating marker forms to slip past a stripper
    /// that handles the forms in separate, non-composing passes.
    #[test]
    fn bracketed_strips_mixed_marker_forms() {
        assert_eq!(encode_paste("a\x1b[201~b\u{9b}200~c", true), wrapped("abc"));
    }

    /// Asserts that markers re-exposed by an earlier removal are also
    /// stripped, for start, end, and cross-form (7-bit removal exposing
    /// a C1 marker) nests.
    ///
    /// Case: the reason the loop must run to a fixed point — one replace
    /// pass over `ESC [ ESC [ 201 ~ 201 ~` leaves a freshly-assembled
    /// `ESC [ 201 ~` behind, which a single-pass stripper would emit
    /// into the frame.
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
    /// Case: legitimate pasted content such as other CSI sequences
    /// (`ESC[202~`), a truncated marker, or the bare digits `200~`. An
    /// over-eager stripper that matches prefixes or ignores the final
    /// byte would corrupt ordinary text.
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
    /// Case: the API's totality. The host layer never fires an empty
    /// paste (its clipboard read filters empty text) and
    /// `OrzmaTerm::write_paste` early-returns on it, so this input is
    /// reachable only by calling the encoder directly — but a caller
    /// that does must still get a frame, not a bare marker fragment.
    #[test]
    fn bracketed_empty_text_emits_well_formed_brackets() {
        assert_eq!(encode_paste("", true), b"\x1b[200~\x1b[201~");
    }

    /// Asserts that newlines and multi-byte text cross the bracketed
    /// path byte-for-byte.
    ///
    /// Case: the point of bracketed paste — the receiving app asked to
    /// see the body verbatim and applies its own newline handling, so
    /// the `\r`/`\n` normalization of the unbracketed path must NOT leak
    /// in here, and non-ASCII content must not be re-encoded.
    #[test]
    fn bracketed_newlines_and_multibyte_pass_through() {
        assert_eq!(encode_paste("a\r\nb\nc\rd", true), wrapped("a\r\nb\nc\rd"));
        assert_eq!(encode_paste("日本語🦀", true), wrapped("日本語🦀"));
    }

    /// Asserts, over the whole adversarial fixture set, that no marker
    /// spelling ever survives into the emitted body.
    ///
    /// Case: the invariant behind every example test above, checked as a
    /// property so a future fixture (or a stripping bug the examples
    /// happen to miss) is still caught: whatever the input, the frame
    /// must be terminable only by the final `ESC[201~` the encoder
    /// itself appends.
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
    /// Case: resource-exhaustion regression for the fixed-point loop —
    /// each iteration reallocates the body, so a pathological input must
    /// not push the loop into quadratic blowup that stalls the UI.
    #[test]
    fn bracketed_large_adversarial_input_terminates() {
        let input = END_7BIT.repeat(16_000);
        assert_eq!(body_of(&encode_paste(&input, true)), "");
    }

    /// Asserts the unbracketed newline rules: `\r\n` collapses to `\r`,
    /// a lone `\n` becomes `\r`, existing `\r` passes through, and a
    /// `\r\n\n` run yields exactly two `\r`.
    ///
    /// Case: a multi-line paste from an editor into a plain shell — the
    /// line discipline expects one `\r` per line, and double-converting
    /// `\r\n` would submit an empty extra line per paste.
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
    /// Case: with DECSET 2004 off there is no frame to break out of, so
    /// the markers are ordinary bytes; the composite input pins that the
    /// two rules (keep markers, normalize newlines) apply independently
    /// to one text.
    #[test]
    fn unbracketed_does_not_strip_paste_markers() {
        assert_eq!(
            encode_paste("x\x1b[201~\ny", false),
            b"x\x1b[201~\ry".to_vec()
        );
    }

    /// Asserts that empty text encodes to zero bytes.
    ///
    /// Case: nothing to paste means nothing on the wire — unlike the
    /// bracketed path there is no frame that must stay well-formed.
    #[test]
    fn unbracketed_empty_text_emits_nothing() {
        assert_eq!(encode_paste("", false), b"");
    }

    /// Asserts that control characters pass through the unbracketed path
    /// unfiltered: TAB, BEL, ESC, ETX, and NUL.
    ///
    /// Case: characterization of the current policy — an unbracketed
    /// paste carries the same authority as typed input, so the encoder
    /// filters nothing beyond newline normalization. A C0/ESC filtering
    /// policy (and any multi-line paste confirmation) is a separate,
    /// host-level security concern, deliberately NOT smuggled in here.
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
