//! Cursor configuration: the `[cursor]` section.

use crate::norm_unit;
use serde::Deserialize;
use std::time::Duration;

/// The shape the caret takes when no program has asked for another.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(from = "String")]
pub enum CursorStyleSetting {
    /// A block filling the cell.
    #[default]
    Block,
    /// A line along the bottom of the cell.
    Underline,
    /// A vertical line at the left of the cell.
    Bar,
}

impl From<String> for CursorStyleSetting {
    fn from(value: String) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "underline" => Self::Underline,
            "bar" => Self::Bar,
            _ => Self::Block,
        }
    }
}

/// Fully-resolved `[cursor]` config block.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CursorConfig {
    /// The caret's shape until a program sets one with `DECSCUSR`.
    pub style: CursorStyleSetting,
    /// Whether a caret in an inactive pane or an unfocused window is
    /// drawn as a hollow block.
    pub unfocused_hollow: bool,
    blink_interval: u64,
    blink_timeout: u64,
    thickness: f32,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: CursorStyleSetting::default(),
            unfocused_hollow: true,
            blink_interval: DEFAULT_BLINK_INTERVAL_MS,
            blink_timeout: DEFAULT_BLINK_TIMEOUT_SECS,
            thickness: DEFAULT_THICKNESS,
        }
    }
}

impl CursorConfig {
    /// The interval between blink phases; `None` when the caret does
    /// not blink. A value below 10 ms is raised to 10 ms.
    pub fn blink_interval(&self) -> Option<Duration> {
        if self.blink_interval == 0 {
            return None;
        }
        Some(Duration::from_millis(
            self.blink_interval.max(MIN_BLINK_INTERVAL_MS),
        ))
    }

    /// How long the caret keeps blinking with no keystroke; `None` when
    /// it blinks indefinitely. A configured timeout is raised to one
    /// full blink cycle.
    pub fn blink_timeout(&self) -> Option<Duration> {
        if self.blink_timeout == 0 {
            return None;
        }
        Some(
            self.blink_interval()
                .unwrap_or(Duration::ZERO)
                .saturating_mul(2)
                .max(Duration::from_secs(self.blink_timeout)),
        )
    }

    /// Caret thickness as a fraction of the cell width, in `0.0..=1.0`;
    /// the default stands in for a NaN.
    pub fn thickness(&self) -> f32 {
        norm_unit(self.thickness, DEFAULT_THICKNESS)
    }
}

const DEFAULT_BLINK_INTERVAL_MS: u64 = 750;
const DEFAULT_BLINK_TIMEOUT_SECS: u64 = 5;
const DEFAULT_THICKNESS: f32 = 0.15;
const MIN_BLINK_INTERVAL_MS: u64 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    fn from_toml(s: &str) -> CursorConfig {
        toml::from_str(s).expect("the fixture parses")
    }

    /// Asserts that an empty section resolves to a block caret with the
    /// shipped blink timings.
    ///
    /// Case: the user has never written a `[cursor]` section.
    #[test]
    fn defaults_are_a_block_with_the_shipped_timings() {
        let cfg = CursorConfig::default();
        assert_eq!(cfg.style, CursorStyleSetting::Block);
        assert!(cfg.unfocused_hollow);
        assert_eq!(cfg.blink_interval(), Some(Duration::from_millis(750)));
        assert_eq!(cfg.blink_timeout(), Some(Duration::from_secs(5)));
        assert_eq!(cfg.thickness(), 0.15);
    }

    /// Asserts that a zero timeout means the caret blinks indefinitely
    /// rather than stopping immediately.
    ///
    /// Case: the user disables the inactivity pause with
    /// `blink_timeout = 0`.
    #[test]
    fn a_zero_timeout_blinks_indefinitely() {
        assert_eq!(from_toml("blink_timeout = 0").blink_timeout(), None);
    }

    /// Asserts that the effective timeout is raised to one full blink
    /// cycle rather than being taken literally.
    ///
    /// Case: the user asks for a one-second pause while the interval is
    /// the default 750 ms.
    #[test]
    fn a_timeout_shorter_than_one_cycle_is_raised() {
        let cfg = from_toml("blink_timeout = 1");
        assert_eq!(cfg.blink_timeout(), Some(Duration::from_millis(1500)));
    }

    /// Asserts that a zero interval reports no blink at all, while any
    /// other value below the floor is raised to it.
    ///
    /// Case: one user turns blinking off with `blink_interval = 0`, and
    /// another writes a 5 ms interval while experimenting.
    #[test]
    fn a_zero_interval_reports_no_blink_and_a_low_one_is_raised() {
        assert_eq!(from_toml("blink_interval = 0").blink_interval(), None);
        assert_eq!(
            from_toml("blink_interval = 5").blink_interval(),
            Some(Duration::from_millis(10))
        );
    }

    /// Asserts that thickness is clamped to the unit range and that NaN
    /// falls back to the default.
    ///
    /// Case: the user writes an out-of-range thickness.
    #[test]
    fn thickness_clamps_and_nan_falls_back() {
        assert_eq!(from_toml("thickness = 4.0").thickness(), 1.0);
        assert_eq!(from_toml("thickness = -1.0").thickness(), 0.0);
        assert_eq!(from_toml("thickness = nan").thickness(), 0.15);
    }

    /// Asserts that an unrecognized `style` word falls back to the
    /// default rather than failing the load.
    ///
    /// Case: the user misspells `underline` as `underlien`.
    #[test]
    fn an_unknown_word_falls_back_to_the_default() {
        let cfg = from_toml("style = \"underlien\"");
        assert_eq!(cfg.style, CursorStyleSetting::Block);
    }

    /// Asserts that a misspelled key inside the section is rejected
    /// rather than ignored.
    ///
    /// Case: the user types `stlye` instead of `style`.
    #[test]
    fn a_misspelled_key_is_rejected() {
        assert!(toml::from_str::<CursorConfig>("stlye = \"bar\"").is_err());
    }
}
