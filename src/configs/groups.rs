//! Bevy `SettingsGroup` resources: the Reflect-native persistence layer for
//! each `[section]` of the user's config, and the `From<&Group> for Raw…`
//! conversions that hand each group's live values to
//! `orzma_configs::RawSettings::resolve` to produce a typed `OrzmaConfigs`.
//! Also holds the reverse `From<&Raw…> for Group` conversions, used by the
//! one-time legacy migration (`src/configs/migrate.rs`) to populate these
//! groups from a `RawSettings` built off the user's old config file.

use bevy::prelude::*;
use bevy::settings::{ReflectSettingsGroup, SettingsGroup};
use orzma_configs::RawSettings;
use orzma_configs::raw::{
    RawFace, RawFont, RawInactivePane, RawKeyboard, RawMouse, RawOrzma, RawScrollback,
    RawShortcuts, RawViMode,
};
use std::collections::HashMap;

/// `[shortcuts]` persistence group: the leader, the two timeout scalars, and
/// a flat action-key -> chord-string binding map.
///
/// `bindings` defaults empty: the built-in default bindings live in the
/// domain layer (`orzma_configs::shortcuts::Shortcuts::default`), not here —
/// `bevy::settings` replaces the whole map field on load, so per-entry
/// override merging must happen downstream in `RawSettings::resolve`.
#[derive(Resource, SettingsGroup, Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "shortcuts")]
pub(crate) struct ShortcutSettings {
    /// The `leader` value, if set (a chord string or bare tap-modifier token).
    pub(crate) leader: Option<String>,
    /// Timeout (ms) for a modifier-tap leader.
    pub(crate) leader_tap_timeout_ms: u64,
    /// Repeat window (ms) for `<Leader:r>` bindings.
    pub(crate) repeat_time_ms: u64,
    /// Action key (kebab-case) -> chord string; empty by default (see above).
    pub(crate) bindings: HashMap<String, String>,
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        ShortcutSettings {
            leader: None,
            leader_tap_timeout_ms: 300,
            repeat_time_ms: 500,
            bindings: HashMap::new(),
        }
    }
}

impl From<&ShortcutSettings> for RawShortcuts {
    fn from(value: &ShortcutSettings) -> Self {
        RawShortcuts {
            leader: value.leader.clone(),
            leader_tap_timeout_ms: value.leader_tap_timeout_ms,
            repeat_time_ms: value.repeat_time_ms,
            bindings: value
                .bindings
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

impl From<&RawShortcuts> for ShortcutSettings {
    fn from(value: &RawShortcuts) -> Self {
        ShortcutSettings {
            leader: value.leader.clone(),
            leader_tap_timeout_ms: value.leader_tap_timeout_ms,
            repeat_time_ms: value.repeat_time_ms,
            bindings: value
                .bindings
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

/// `[vi-mode]` persistence group: action key -> list of key strings.
///
/// `bindings` defaults empty for the same reason as `ShortcutSettings::bindings`.
#[derive(Resource, SettingsGroup, Reflect, Clone, Debug, Default)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "vi-mode")]
pub(crate) struct ViModeSettings {
    /// Action key -> key strings (empty list unbinds the action).
    pub(crate) bindings: HashMap<String, Vec<String>>,
}

impl From<&ViModeSettings> for RawViMode {
    fn from(value: &ViModeSettings) -> Self {
        RawViMode {
            bindings: value
                .bindings
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

impl From<&RawViMode> for ViModeSettings {
    fn from(value: &RawViMode) -> Self {
        ViModeSettings {
            bindings: value
                .bindings
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

/// One `[font]` face: family and style, both optional. A Reflect-native
/// mirror of `orzma_configs::font::FontFaceConfig`, kept as a distinct type:
/// this one is the SettingsGroup's own field type, not the domain crate's
/// serde-based type.
#[derive(Reflect, Clone, Debug, Default)]
pub(crate) struct FontFaceConfig {
    /// Font-family name.
    pub(crate) family: Option<String>,
    /// Alacritty-style style string.
    pub(crate) style: Option<String>,
}

impl From<&FontFaceConfig> for RawFace {
    fn from(value: &FontFaceConfig) -> Self {
        RawFace {
            family: value.family.clone(),
            style: value.style.clone(),
        }
    }
}

impl From<&RawFace> for FontFaceConfig {
    fn from(value: &RawFace) -> Self {
        FontFaceConfig {
            family: value.family.clone(),
            style: value.style.clone(),
        }
    }
}

/// `[font]` persistence group: size plus the four terminal faces.
#[derive(Resource, SettingsGroup, Reflect, Clone, Debug)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "font")]
pub(crate) struct FontSettings {
    /// Terminal font size in logical pixels.
    pub(crate) size: f32,
    /// The regular face.
    pub(crate) normal: FontFaceConfig,
    /// The bold face.
    pub(crate) bold: FontFaceConfig,
    /// The italic face.
    pub(crate) italic: FontFaceConfig,
    /// The bold-italic face.
    pub(crate) bold_italic: FontFaceConfig,
}

impl Default for FontSettings {
    fn default() -> Self {
        FontSettings {
            size: 11.25,
            normal: FontFaceConfig::default(),
            bold: FontFaceConfig::default(),
            italic: FontFaceConfig::default(),
            bold_italic: FontFaceConfig::default(),
        }
    }
}

impl From<&FontSettings> for RawFont {
    fn from(value: &FontSettings) -> Self {
        RawFont {
            size: value.size,
            normal: (&value.normal).into(),
            bold: (&value.bold).into(),
            italic: (&value.italic).into(),
            bold_italic: (&value.bold_italic).into(),
        }
    }
}

impl From<&RawFont> for FontSettings {
    fn from(value: &RawFont) -> Self {
        FontSettings {
            size: value.size,
            normal: (&value.normal).into(),
            bold: (&value.bold).into(),
            italic: (&value.italic).into(),
            bold_italic: (&value.bold_italic).into(),
        }
    }
}

/// `[mouse]` persistence group, mirroring `RawMouse` field-for-field. The
/// enum field (`fine_modifier`) is carried as a `String`: bevy_reflect
/// serializes a real `#[derive(Reflect)] enum` field as its PascalCase Rust
/// variant name, not the lowercase form `MouseConfig`'s serde attribute
/// produces, so the enum is parsed downstream in `RawSettings::resolve`
/// instead.
#[derive(Resource, SettingsGroup, Reflect, Clone, Debug)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "mouse")]
pub(crate) struct MouseSettings {
    /// Lines scrolled per notch.
    pub(crate) lines_per_notch: u32,
    /// Which modifier activates fine scrolling.
    pub(crate) fine_modifier: String,
    /// Lines scrolled per notch when the fine modifier is held.
    pub(crate) fine_lines: u32,
    /// Upper bound on mouse-protocol events emitted per frame.
    pub(crate) max_protocol_events_per_frame: u32,
    /// Wheel-input accumulation threshold in cells per notch.
    pub(crate) cells_per_notch: f32,
    /// Dominant-axis lock strength for trackpad scrolling.
    pub(crate) axis_lock_ratio: f32,
    /// Max gap (ms) between clicks counted as a double/triple click.
    pub(crate) double_click_timeout_ms: u32,
    /// Max cursor drift (px) between clicks counted as the same chord.
    pub(crate) click_drift_px: f32,
    /// Drag-scroll tick rate (ms) at the pane edge.
    pub(crate) autoscroll_base_period_ms: u32,
    /// Hard floor (ms) on the drag-scroll rate.
    pub(crate) autoscroll_min_period_ms: u32,
    /// Linear decrement (ms per cell past the edge).
    pub(crate) autoscroll_step_ms: u32,
    /// Pointer travel (px) before a left-press is treated as a drag.
    pub(crate) drag_threshold_px: f32,
    /// Half-width (px) of a pane divider's grab zone for resize.
    pub(crate) divider_grab_tolerance_px: f32,
}

impl Default for MouseSettings {
    fn default() -> Self {
        MouseSettings {
            lines_per_notch: 3,
            fine_modifier: "alt".to_string(),
            fine_lines: 1,
            max_protocol_events_per_frame: 8,
            cells_per_notch: 0.5,
            axis_lock_ratio: 0.9,
            double_click_timeout_ms: 400,
            click_drift_px: 8.0,
            autoscroll_base_period_ms: 50,
            autoscroll_min_period_ms: 16,
            autoscroll_step_ms: 4,
            drag_threshold_px: 4.0,
            divider_grab_tolerance_px: 4.0,
        }
    }
}

impl From<&MouseSettings> for RawMouse {
    fn from(value: &MouseSettings) -> Self {
        RawMouse {
            lines_per_notch: value.lines_per_notch,
            fine_modifier: value.fine_modifier.clone(),
            fine_lines: value.fine_lines,
            max_protocol_events_per_frame: value.max_protocol_events_per_frame,
            cells_per_notch: value.cells_per_notch,
            axis_lock_ratio: value.axis_lock_ratio,
            double_click_timeout_ms: value.double_click_timeout_ms,
            click_drift_px: value.click_drift_px,
            autoscroll_base_period_ms: value.autoscroll_base_period_ms,
            autoscroll_min_period_ms: value.autoscroll_min_period_ms,
            autoscroll_step_ms: value.autoscroll_step_ms,
            drag_threshold_px: value.drag_threshold_px,
            divider_grab_tolerance_px: value.divider_grab_tolerance_px,
        }
    }
}

impl From<&RawMouse> for MouseSettings {
    fn from(value: &RawMouse) -> Self {
        MouseSettings {
            lines_per_notch: value.lines_per_notch,
            fine_modifier: value.fine_modifier.clone(),
            fine_lines: value.fine_lines,
            max_protocol_events_per_frame: value.max_protocol_events_per_frame,
            cells_per_notch: value.cells_per_notch,
            axis_lock_ratio: value.axis_lock_ratio,
            double_click_timeout_ms: value.double_click_timeout_ms,
            click_drift_px: value.click_drift_px,
            autoscroll_base_period_ms: value.autoscroll_base_period_ms,
            autoscroll_min_period_ms: value.autoscroll_min_period_ms,
            autoscroll_step_ms: value.autoscroll_step_ms,
            drag_threshold_px: value.drag_threshold_px,
            divider_grab_tolerance_px: value.divider_grab_tolerance_px,
        }
    }
}

/// `[keyboard]` persistence group: which Option key(s) act as Meta on macOS.
/// `option_as_alt` is carried as a `String` for the same reason as
/// `MouseSettings.fine_modifier`.
#[derive(Resource, SettingsGroup, Reflect, Clone, Debug)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "keyboard")]
pub(crate) struct KeyboardSettings {
    /// `option_as_alt` value (`"none"` / `"left"` / `"right"` / `"both"`).
    pub(crate) option_as_alt: String,
}

impl Default for KeyboardSettings {
    fn default() -> Self {
        KeyboardSettings {
            option_as_alt: "none".to_string(),
        }
    }
}

impl From<&KeyboardSettings> for RawKeyboard {
    fn from(value: &KeyboardSettings) -> Self {
        RawKeyboard {
            option_as_alt: value.option_as_alt.clone(),
        }
    }
}

impl From<&RawKeyboard> for KeyboardSettings {
    fn from(value: &RawKeyboard) -> Self {
        KeyboardSettings {
            option_as_alt: value.option_as_alt.clone(),
        }
    }
}

/// `[inactive_pane]` persistence group, mirroring `RawInactivePane`
/// field-for-field.
#[derive(Resource, SettingsGroup, Reflect, Clone, Debug)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "inactive_pane")]
pub(crate) struct InactivePaneSettings {
    /// Whether inactive panes are treated at all.
    pub(crate) enabled: bool,
    /// Inactive-pane brightness multiplier.
    pub(crate) dim: f32,
    /// Background-tint target color as a `#RRGGBB` hex string.
    pub(crate) tint_color: String,
    /// Background-tint strength.
    pub(crate) tint: f32,
    /// Inactive-webview brightness multiplier.
    pub(crate) webview_dim: f32,
    /// Inactive-webview desaturation.
    pub(crate) webview_desaturate: f32,
}

impl Default for InactivePaneSettings {
    fn default() -> Self {
        InactivePaneSettings {
            enabled: true,
            dim: 1.0,
            tint_color: "#3a3b45".to_string(),
            tint: 0.85,
            webview_dim: 0.55,
            webview_desaturate: 0.6,
        }
    }
}

impl From<&InactivePaneSettings> for RawInactivePane {
    fn from(value: &InactivePaneSettings) -> Self {
        RawInactivePane {
            enabled: value.enabled,
            dim: value.dim,
            tint_color: value.tint_color.clone(),
            tint: value.tint,
            webview_dim: value.webview_dim,
            webview_desaturate: value.webview_desaturate,
        }
    }
}

impl From<&RawInactivePane> for InactivePaneSettings {
    fn from(value: &RawInactivePane) -> Self {
        InactivePaneSettings {
            enabled: value.enabled,
            dim: value.dim,
            tint_color: value.tint_color.clone(),
            tint: value.tint,
            webview_dim: value.webview_dim,
            webview_desaturate: value.webview_desaturate,
        }
    }
}

/// `[orzma]` persistence group: single-terminal mode configuration.
#[derive(Resource, SettingsGroup, Reflect, Clone, Debug, Default)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "orzma")]
pub(crate) struct OrzmaSettings {
    /// Shell program to launch. `None` resolves at runtime via `$SHELL`.
    pub(crate) shell: Option<String>,
}

impl From<&OrzmaSettings> for RawOrzma {
    fn from(value: &OrzmaSettings) -> Self {
        RawOrzma {
            shell: value.shell.clone(),
        }
    }
}

impl From<&RawOrzma> for OrzmaSettings {
    fn from(value: &RawOrzma) -> Self {
        OrzmaSettings {
            shell: value.shell.clone(),
        }
    }
}

/// `[scrollback]` persistence group.
#[derive(Resource, SettingsGroup, Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "scrollback")]
pub(crate) struct ScrollbackSettings {
    /// Lines of tmux history to fetch and seed on attach.
    pub(crate) seed_lines: usize,
}

impl Default for ScrollbackSettings {
    fn default() -> Self {
        ScrollbackSettings { seed_lines: 2000 }
    }
}

impl From<&ScrollbackSettings> for RawScrollback {
    fn from(value: &ScrollbackSettings) -> Self {
        RawScrollback {
            seed_lines: value.seed_lines,
        }
    }
}

impl From<&RawScrollback> for ScrollbackSettings {
    fn from(value: &RawScrollback) -> Self {
        ScrollbackSettings {
            seed_lines: value.seed_lines,
        }
    }
}

/// Reads each settings group `Resource` from `world`, falling back to the
/// group's `Default` when absent — so a world without `SettingsPlugin`
/// resolves to the domain defaults (hermetic for tests). Called by
/// `resolve_and_insert` (`src/configs.rs`) in the real load path, and
/// directly by this file's tests.
pub(crate) fn collect_raw(world: &World) -> RawSettings {
    RawSettings {
        shortcuts: group_raw::<ShortcutSettings, RawShortcuts>(world),
        vi_mode: group_raw::<ViModeSettings, RawViMode>(world),
        font: group_raw::<FontSettings, RawFont>(world),
        mouse: group_raw::<MouseSettings, RawMouse>(world),
        keyboard: group_raw::<KeyboardSettings, RawKeyboard>(world),
        inactive_pane: group_raw::<InactivePaneSettings, RawInactivePane>(world),
        orzma: group_raw::<OrzmaSettings, RawOrzma>(world),
        scrollback: group_raw::<ScrollbackSettings, RawScrollback>(world),
    }
}

/// Reads settings-group `Resource` `G` from `world` (falling back to
/// `G::default()` when absent) and converts it to its raw counterpart `R`
/// via `R: From<&G>`. Only borrows `G` — never clones the whole resource —
/// so the sole clones that happen are the individual fields each `From`
/// impl actually needs. Shared by every arm of `collect_raw` to avoid
/// repeating the same `.cloned().unwrap_or_default()` dance eight times.
fn group_raw<G, R>(world: &World) -> R
where
    G: Resource + Default,
    for<'a> R: From<&'a G>,
{
    match world.get_resource::<G>() {
        Some(g) => R::from(g),
        None => R::from(&G::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_configs::OrzmaConfigs;

    #[test]
    fn default_groups_convert_to_default_raw() {
        let raw = RawSettings {
            shortcuts: (&ShortcutSettings::default()).into(),
            vi_mode: (&ViModeSettings::default()).into(),
            font: (&FontSettings::default()).into(),
            mouse: (&MouseSettings::default()).into(),
            keyboard: (&KeyboardSettings::default()).into(),
            inactive_pane: (&InactivePaneSettings::default()).into(),
            orzma: (&OrzmaSettings::default()).into(),
            scrollback: (&ScrollbackSettings::default()).into(),
        };
        let (cfg, diags) = raw.resolve();
        assert!(
            diags.is_empty(),
            "default config must resolve cleanly: {diags:?}"
        );
        assert_eq!(cfg, OrzmaConfigs::default());
    }

    #[test]
    fn collect_raw_falls_back_to_group_defaults_when_world_has_no_settings() {
        let world = World::new();
        let raw = collect_raw(&world);
        assert_eq!(raw, RawSettings::default());
    }

    #[test]
    fn shortcut_settings_round_trips_through_raw() {
        let mut original = ShortcutSettings {
            leader: Some("Ctrl+A".to_string()),
            leader_tap_timeout_ms: 900,
            repeat_time_ms: 42,
            ..ShortcutSettings::default()
        };
        original
            .bindings
            .insert("quit".to_string(), "Cmd+Shift+Q".to_string());
        let raw: RawShortcuts = (&original).into();
        let round_tripped: ShortcutSettings = (&raw).into();
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn scrollback_settings_round_trips_through_raw() {
        let original = ScrollbackSettings { seed_lines: 12345 };
        let raw: RawScrollback = (&original).into();
        let round_tripped: ScrollbackSettings = (&raw).into();
        assert_eq!(original, round_tripped);
    }
}
