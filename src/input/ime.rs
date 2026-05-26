//! IME composition state for the terminal overlay.
//!
//! Provides `Composition`, a validated snapshot of a preedit string and
//! its UTF-8-safe caret offset. Bevy window event handling and
//! `Ime::Commit` forwarding are added in later tasks.

#[derive(Debug)]
pub(crate) struct Composition {
    text: String,
    caret: Option<usize>,
}

impl Composition {
    /// Validates and constructs a `Composition`. Returns `None` when:
    ///   - `text` is empty (treat any empty-value Preedit as
    ///     "no composition");
    ///   - `raw_caret.0` is out of bounds or lands on a non-UTF-8
    ///     boundary byte (defensive: winit returns byte offsets that
    ///     we later slice into).
    ///
    /// Only honors `raw_caret.0` (the begin offset); the selection
    /// range is out of scope per the design spec, Decision 3.
    pub(crate) fn try_new(text: String, raw_caret: Option<(usize, usize)>) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let caret = match raw_caret {
            None => None,
            Some((begin, _end)) => {
                if begin <= text.len() && text.is_char_boundary(begin) {
                    Some(begin)
                } else {
                    None
                }
            }
        };
        Some(Composition { text, caret })
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn caret(&self) -> Option<usize> {
        self.caret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_new_returns_none_for_empty_text() {
        assert!(Composition::try_new(String::new(), None).is_none());
        assert!(Composition::try_new(String::new(), Some((0, 0))).is_none());
    }

    #[test]
    fn try_new_accepts_valid_caret() {
        let c = Composition::try_new("hello".into(), Some((3, 3))).unwrap();
        assert_eq!(c.text(), "hello");
        assert_eq!(c.caret(), Some(3));
    }

    #[test]
    fn try_new_accepts_caret_at_text_len() {
        let c = Composition::try_new("ab".into(), Some((2, 2))).unwrap();
        assert_eq!(c.caret(), Some(2));
    }

    #[test]
    fn try_new_clamps_out_of_bounds_caret_to_none() {
        let c = Composition::try_new("ab".into(), Some((99, 99))).unwrap();
        assert_eq!(c.text(), "ab");
        assert_eq!(c.caret(), None);
    }

    #[test]
    fn try_new_rejects_non_char_boundary_caret() {
        // "あ" is 3 bytes in UTF-8; byte 1 is mid-char.
        let c = Composition::try_new("あ".into(), Some((1, 1))).unwrap();
        assert_eq!(c.text(), "あ");
        assert_eq!(c.caret(), None);
    }

    #[test]
    fn try_new_honors_only_begin_offset() {
        // The end offset is ignored per Decision 3 (no selection range).
        let c = Composition::try_new("hello".into(), Some((2, 5))).unwrap();
        assert_eq!(c.caret(), Some(2));
    }

    #[test]
    fn try_new_with_none_caret_keeps_none() {
        let c = Composition::try_new("hi".into(), None).unwrap();
        assert_eq!(c.caret(), None);
    }
}
