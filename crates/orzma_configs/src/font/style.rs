//! Parser for Alacritty-style `style` strings into a font weight + slant.

use std::str::FromStr;

/// A parsed font `style` string: an OpenType weight (100–950) plus slant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontStyleSpec {
    /// Numeric weight on the CSS/OpenType scale (100–950); 400 = Regular, 700 = Bold.
    pub weight: u16,
    /// Slant selector for the face.
    pub slant: FontSlant,
}

/// Slant component of a parsed `style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSlant {
    /// Upright.
    Normal,
    /// Italic.
    Italic,
    /// Oblique (slanted upright design).
    Oblique,
}

/// A `style` string that contained a token matching neither a weight nor a
/// slant name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidFontStyleToken {
    /// The offending token, as written by the user.
    pub token: String,
}

impl FromStr for FontStyleSpec {
    type Err = InvalidFontStyleToken;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Collapse away spaces and hyphens so "Extra Bold", "extra-bold", and
        // "ExtraBold" all reduce to one token, and "Bold Italic" / "BoldItalic"
        // both become "bolditalic".
        let normalized: String = s
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .flat_map(char::to_lowercase)
            .collect();
        let (rest, slant) = FontSlant::strip(&normalized);
        let weight = if rest.is_empty() {
            400
        } else {
            weight_name(rest).ok_or_else(|| InvalidFontStyleToken {
                token: s.trim().to_string(),
            })?
        };
        Ok(Self { weight, slant })
    }
}

fn weight_name(word: &str) -> Option<u16> {
    Some(match word {
        "thin" | "hairline" => 100,
        "extralight" | "ultralight" => 200,
        "light" => 300,
        "regular" | "normal" | "book" => 400,
        "medium" => 500,
        "semibold" | "demibold" => 600,
        "bold" => 700,
        "extrabold" | "ultrabold" => 800,
        "black" | "heavy" => 900,
        _ => return None,
    })
}

impl FontSlant {
    /// Strips a leading or trailing slant name (`italic` / `oblique`) from the
    /// normalized `token`, returning the remaining weight portion and the slant
    /// (`Normal` when none is present).
    fn strip(token: &str) -> (&str, Self) {
        for (name, slant) in [("italic", Self::Italic), ("oblique", Self::Oblique)] {
            if let Some(rest) = token.strip_suffix(name) {
                return (rest, slant);
            }
            if let Some(rest) = token.strip_prefix(name) {
                return (rest, slant);
            }
        }
        (token, Self::Normal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(s: &str) -> FontStyleSpec {
        FontStyleSpec::from_str(s).expect("valid style")
    }

    #[test]
    fn parses_standard_four() {
        assert_eq!(
            spec("Regular"),
            FontStyleSpec {
                weight: 400,
                slant: FontSlant::Normal
            }
        );
        assert_eq!(
            spec("Bold"),
            FontStyleSpec {
                weight: 700,
                slant: FontSlant::Normal
            }
        );
        assert_eq!(
            spec("Italic"),
            FontStyleSpec {
                weight: 400,
                slant: FontSlant::Italic
            }
        );
        assert_eq!(
            spec("Bold Italic"),
            FontStyleSpec {
                weight: 700,
                slant: FontSlant::Italic
            }
        );
    }

    #[test]
    fn parses_common_weight_names_case_insensitively() {
        assert_eq!(spec("semibold").weight, 600);
        assert_eq!(spec("SemiBold").weight, 600);
        assert_eq!(spec("DemiBold").weight, 600);
        assert_eq!(
            spec("Medium Italic"),
            FontStyleSpec {
                weight: 500,
                slant: FontSlant::Italic
            }
        );
        assert_eq!(spec("thin").weight, 100);
        assert_eq!(spec("Black").weight, 900);
    }

    #[test]
    fn accepts_hyphenated_concatenated_and_space_separated_weight_names() {
        assert_eq!(spec("extra-bold").weight, 800);
        assert_eq!(spec("ExtraBold").weight, 800);
        assert_eq!(spec("ultra-light").weight, 200);
        // Space-separated multi-word weight names, as macOS Font Book displays.
        assert_eq!(spec("Extra Bold").weight, 800);
        assert_eq!(spec("Semi Bold").weight, 600);
        assert_eq!(spec("Ultra Light").weight, 200);
    }

    #[test]
    fn accepts_concatenated_and_multiword_weight_slant() {
        assert_eq!(
            spec("BoldItalic"),
            FontStyleSpec {
                weight: 700,
                slant: FontSlant::Italic
            }
        );
        assert_eq!(
            spec("Extra Bold Italic"),
            FontStyleSpec {
                weight: 800,
                slant: FontSlant::Italic
            }
        );
    }

    #[test]
    fn slant_only_and_weight_only_default_the_other() {
        assert_eq!(
            spec("Oblique"),
            FontStyleSpec {
                weight: 400,
                slant: FontSlant::Oblique
            }
        );
        assert_eq!(
            spec("Medium"),
            FontStyleSpec {
                weight: 500,
                slant: FontSlant::Normal
            }
        );
    }

    #[test]
    fn empty_string_is_regular() {
        assert_eq!(
            spec(""),
            FontStyleSpec {
                weight: 400,
                slant: FontSlant::Normal
            }
        );
    }

    #[test]
    fn unknown_token_errors_and_names_the_token() {
        let err = FontStyleSpec::from_str("Blod").unwrap_err();
        assert_eq!(err.token, "Blod");
    }
}
