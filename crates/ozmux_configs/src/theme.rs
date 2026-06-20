//! Theme tokens for ozmux UI color configuration.

use serde::{Deserialize, Serialize};

/// Fully-resolved theme: five semantic color tokens that ozmux exposes to the UI.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Theme {
    /// `--color-background` token.
    pub background: String,
    /// `--color-foreground` token.
    pub foreground: String,
    /// `--color-accent` token.
    pub accent: String,
    /// `--color-border` token.
    pub border: String,
    /// `--color-destructive` token.
    pub destructive: String,
}

impl Default for Theme {
    fn default() -> Self {
        // NOTE: values mirror Layer 1 raw tokens in `src/theme.rs` (Bevy UI palette).
        Self {
            background: "#1a1b26".into(),
            foreground: "#c0caf5".into(),
            accent: "#414868".into(),
            border: "#414868".into(),
            destructive: "#f7768e".into(),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_theme_fills_missing_from_default() {
        let t: Theme = toml::from_str(r##"accent = "#abcdef""##).unwrap();
        assert_eq!(t.accent, "#abcdef");
        assert_eq!(t.background, Theme::default().background);
        assert_eq!(t.destructive, Theme::default().destructive);
    }

    #[test]
    fn empty_theme_is_default() {
        let t: Theme = toml::from_str("").unwrap();
        assert_eq!(t, Theme::default());
    }
}
