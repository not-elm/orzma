//! `TerminalTitle` Component + OSC title sanitization.
//!
//! OSC 0/2 title content is fully attacker-controlled. `sanitize_title`
//! strips C0/C1 control characters, zero-width and bidi-override
//! characters, and caps the length so a hostile title cannot corrupt
//! or spoof the tab bar.

use bevy::prelude::Component;

/// Per-entity terminal title. `None` before any title has been set
/// or after `ResetTitle`.
#[derive(Component, Default, Debug, Clone)]
pub struct TerminalTitle(pub Option<String>);

/// Maximum length (in `char`s) of a sanitized title.
const MAX_LEN: usize = 256;

/// Returns a display-safe copy of an OSC terminal title.
pub fn sanitize_title(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| !is_disallowed(*c)).collect();
    if cleaned.chars().count() > MAX_LEN {
        cleaned
            .chars()
            .take(MAX_LEN - 1)
            .chain(std::iter::once('…'))
            .collect()
    } else {
        cleaned
    }
}

fn is_disallowed(c: char) -> bool {
    let u = c as u32;
    u <= 0x1F
        || (0x7F..=0x9F).contains(&u)
        || matches!(
            c,
            '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

#[cfg(test)]
mod tests {
    use super::sanitize_title;

    #[test]
    fn strips_control_chars() {
        assert_eq!(sanitize_title("hi\x07\x1bthere"), "hithere");
        assert_eq!(sanitize_title("clean title"), "clean title");
    }

    #[test]
    fn strips_bidi_override() {
        assert_eq!(sanitize_title("a\u{202e}b"), "ab");
    }

    #[test]
    fn caps_length_with_ellipsis() {
        let out = sanitize_title(&"x".repeat(500));
        assert_eq!(out.chars().count(), 256);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn short_title_unchanged() {
        assert_eq!(sanitize_title(&"x".repeat(256)).chars().count(), 256);
    }
}
