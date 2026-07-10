//! `Raw*` types: the plain, always-parseable shape every `[section]` of the
//! user's TOML config deserializes into before `RawSettings::resolve`
//! converts it into the fully-typed `OrzmaConfigs`.

use std::collections::BTreeMap;

/// All `Raw*` sections mirroring the user-facing TOML shape. Deserialized
/// directly (or built up field-by-field) from the user's config, then fed to
/// [`crate::resolve`] to produce a typed `OrzmaConfigs` plus diagnostics.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RawSettings {
    /// `[shortcuts]` section.
    pub shortcuts: RawShortcuts,
    /// `[vi-mode]` section.
    pub vi_mode: RawViMode,
    /// `[font]` section.
    pub font: RawFont,
    /// `[mouse]` section.
    pub mouse: RawMouse,
    /// `[keyboard]` section.
    pub keyboard: RawKeyboard,
    /// `[inactive_pane]` section.
    pub inactive_pane: RawInactivePane,
    /// `[orzma]` section.
    pub orzma: RawOrzma,
    /// `[scrollback]` section.
    pub scrollback: RawScrollback,
}

/// `[shortcuts]` section: the leader, the two timeout scalars, and a flat
/// action-key -> chord-string map re-emitted as a `toml::Table` and fed to
/// the existing `Shortcuts` deserializer by `RawSettings::resolve`.
#[derive(Clone, Debug, PartialEq)]
pub struct RawShortcuts {
    /// The `leader` value, if set (a chord string or bare tap-modifier token).
    pub leader: Option<String>,
    /// `leader-tap-timeout-ms`. Mirrors `Shortcuts::leader_tap_timeout_ms` (`u64`).
    pub leader_tap_timeout_ms: u64,
    /// `repeat-time-ms`. Mirrors `Shortcuts::repeat_time_ms` (`u64`).
    pub repeat_time_ms: u64,
    /// Action key (kebab-case) -> chord string, one entry per user-set
    /// binding. Omitted actions keep their built-in default.
    pub bindings: BTreeMap<String, String>,
}

impl Default for RawShortcuts {
    // NOTE: derived `Default` would set `repeat_time_ms = 0`, which disables
    // key-repeat outright; these values must mirror `Shortcuts::default()`
    // (`shortcuts.rs`) exactly.
    fn default() -> Self {
        RawShortcuts {
            leader: None,
            leader_tap_timeout_ms: 300,
            repeat_time_ms: 500,
            bindings: BTreeMap::new(),
        }
    }
}

/// `[vi-mode]` section: action key (kebab-case) -> list of key strings.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RawViMode {
    /// Action key -> key strings (empty list unbinds the action).
    pub bindings: BTreeMap<String, Vec<String>>,
}

/// One `[font]` face: family and style, both optional (mirrors `FontFaceConfig`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RawFace {
    /// Font-family name.
    pub family: Option<String>,
    /// Alacritty-style style string.
    pub style: Option<String>,
}

/// `[font]` section: size plus the four terminal faces.
#[derive(Clone, Debug, PartialEq)]
pub struct RawFont {
    /// Terminal font size in logical pixels.
    pub size: f32,
    /// The regular face.
    pub normal: RawFace,
    /// The bold face.
    pub bold: RawFace,
    /// The italic face.
    pub italic: RawFace,
    /// The bold-italic face.
    pub bold_italic: RawFace,
}

impl Default for RawFont {
    // NOTE: derived `Default` would set `size = 0.0`, which fails
    // `FontConfig` validation (`0 < size <= 200`); mirror `FontConfig`'s
    // default size (`font.rs`) exactly.
    fn default() -> Self {
        RawFont {
            size: 11.25,
            normal: RawFace::default(),
            bold: RawFace::default(),
            italic: RawFace::default(),
            bold_italic: RawFace::default(),
        }
    }
}

/// `[mouse]` section, mirroring `MouseConfig` field-for-field (the enum
/// field is carried as a `String` pending `resolve`'s parse).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RawMouse {
    /// Lines scrolled per notch.
    pub lines_per_notch: u32,
    /// Which modifier activates fine scrolling.
    pub fine_modifier: String,
    /// Lines scrolled per notch when the fine modifier is held.
    pub fine_lines: u32,
    /// Upper bound on mouse-protocol events emitted per frame.
    pub max_protocol_events_per_frame: u32,
    /// Wheel-input accumulation threshold in cells per notch.
    pub cells_per_notch: f32,
    /// Dominant-axis lock strength for trackpad scrolling.
    pub axis_lock_ratio: f32,
    /// Max gap (ms) between clicks counted as a double/triple click.
    pub double_click_timeout_ms: u32,
    /// Max cursor drift (px) between clicks counted as the same chord.
    pub click_drift_px: f32,
    /// Drag-scroll tick rate (ms) at the pane edge.
    pub autoscroll_base_period_ms: u32,
    /// Hard floor (ms) on the drag-scroll rate.
    pub autoscroll_min_period_ms: u32,
    /// Linear decrement (ms per cell past the edge).
    pub autoscroll_step_ms: u32,
    /// Pointer travel (px) before a left-press is treated as a drag.
    pub drag_threshold_px: f32,
    /// Half-width (px) of a pane divider's grab zone for resize.
    pub divider_grab_tolerance_px: f32,
}

/// `[keyboard]` section: which Option key(s) act as Meta on macOS.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RawKeyboard {
    /// `option_as_alt` value (`"none"` / `"left"` / `"right"` / `"both"`).
    pub option_as_alt: String,
}

/// `[inactive_pane]` section, mirroring `InactivePaneConfig` field-for-field.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RawInactivePane {
    /// Whether inactive panes are treated at all.
    pub enabled: bool,
    /// Inactive-pane brightness multiplier.
    pub dim: f32,
    /// Background-tint target color as a `#RRGGBB` hex string.
    pub tint_color: String,
    /// Background-tint strength.
    pub tint: f32,
    /// Inactive-webview brightness multiplier.
    pub webview_dim: f32,
    /// Inactive-webview desaturation.
    pub webview_desaturate: f32,
}

/// `[orzma]` section: single-terminal mode configuration.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RawOrzma {
    /// Shell program to launch. `None` resolves at runtime via `$SHELL`.
    pub shell: Option<String>,
}

/// `[scrollback]` section.
#[derive(Clone, Debug, PartialEq)]
pub struct RawScrollback {
    /// Lines of tmux history to fetch and seed on attach.
    pub seed_lines: usize,
}

impl Default for RawScrollback {
    // NOTE: derived `Default` would set `seed_lines = 0`, silently disabling
    // scrollback seeding; mirror `ScrollbackConfig`'s default (`scrollback.rs`).
    fn default() -> Self {
        RawScrollback { seed_lines: 2000 }
    }
}
