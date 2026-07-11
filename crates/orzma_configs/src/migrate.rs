//! One-time, presence-level conversion of a legacy `~/.config/orzma/config.toml`
//! (named-field `[shortcuts]`, `[vi-mode]` string-or-array, kebab-case scalar
//! keys) into a [`RawSettings`], the same raw model the `bevy::settings`
//! groups feed into [`crate::resolve`]. "Presence-level" means only the keys
//! the user actually set are copied — an omitted action must NOT be frozen as
//! an explicit override, since the resolver fills omitted actions from the
//! built-in defaults.

use crate::raw::{
    RawFace, RawFont, RawInactivePane, RawKeyboard, RawMouse, RawOrzma, RawScrollback, RawSettings,
    RawShortcuts, RawViMode,
};
use crate::resolve::{Diagnostic, Severity};
use crate::shortcuts::SHORTCUT_ACTION_KEYS;
use toml::Value;
use toml::value::Table;

/// Top-level section names the legacy loader understood. Diffed against the
/// document's actual top-level keys so a misspelled section (e.g.
/// `[shortucts]`) warns instead of being silently ignored.
const TOP_LEVEL_SECTIONS: &[&str] = &[
    "shortcuts",
    "vi-mode",
    "font",
    "mouse",
    "keyboard",
    "inactive_pane",
    "orzma",
    "scrollback",
];

/// `[shortcuts]` keys that are NOT action bindings.
const SHORTCUTS_SCALAR_KEYS: &[&str] = &["leader", "leader-tap-timeout-ms", "repeat-time-ms"];

/// `[font]` top-level keys.
const FONT_KEYS: &[&str] = &["size", "normal", "bold", "italic", "bold_italic", "ui"];

/// `[font.<face>]` keys.
const FONT_FACE_KEYS: &[&str] = &["family", "style"];

/// `[mouse]` keys.
const MOUSE_KEYS: &[&str] = &[
    "lines_per_notch",
    "fine_modifier",
    "fine_lines",
    "max_protocol_events_per_frame",
    "cells_per_notch",
    "axis_lock_ratio",
    "double_click_timeout_ms",
    "click_drift_px",
    "autoscroll_base_period_ms",
    "autoscroll_min_period_ms",
    "autoscroll_step_ms",
    "drag_threshold_px",
    "divider_grab_tolerance_px",
];

/// `[keyboard]` keys.
const KEYBOARD_KEYS: &[&str] = &["option_as_alt"];

/// `[inactive_pane]` keys.
const INACTIVE_PANE_KEYS: &[&str] = &[
    "enabled",
    "dim",
    "tint_color",
    "tint",
    "webview_dim",
    "webview_desaturate",
];

/// `[orzma]` keys.
const ORZMA_KEYS: &[&str] = &["shell"];

/// `[scrollback]` keys.
const SCROLLBACK_KEYS: &[&str] = &["seed-lines"];

impl RawSettings {
    /// Converts legacy config TOML text into a [`RawSettings`] at the
    /// presence level: a section is parsed field-by-field from a raw
    /// `toml::Table` (not the typed, `deny_unknown_fields` legacy structs),
    /// so unknown or malformed entries within an otherwise-valid document are
    /// skipped rather than rejecting the whole file. Alongside the
    /// `RawSettings`, returns one `Warn` diagnostic per unrecognized
    /// top-level section or fixed-schema section key (e.g. a misspelled
    /// `[shortucts]` section or a `[mouse]` `lines_pernotch` typo) — the old
    /// `deny_unknown_fields` legacy loader rejected the whole file for a
    /// single typo like this; this reproduces that same signal as a
    /// non-fatal warning instead of silently dropping it.
    ///
    /// Returns `Err` only when `text` itself is not valid TOML. Callers MUST
    /// treat an `Err` as "migration skipped, retry later" — never persist
    /// `RawSettings::default()` in its place, since that would silently and
    /// permanently discard the user's real (just currently unparseable)
    /// config.
    pub fn from_legacy_toml(text: &str) -> Result<(RawSettings, Vec<Diagnostic>), toml::de::Error> {
        let table: Table = toml::from_str(text)?;
        let mut diags = Vec::new();
        warn_unknown_top_level_sections(&mut diags, &table);
        if let Some(t) = section(&table, "shortcuts") {
            // `[shortcuts]` mixes fixed scalar keys with an open set of
            // action-name keys (an action typo is separately caught
            // downstream, in `RawSettings::resolve`'s
            // `SHORTCUT_ACTION_KEYS.contains` check) — a real action key
            // like `quit` must not be flagged as unknown here, so the known
            // set is the union of both.
            let known: Vec<&str> = SHORTCUTS_SCALAR_KEYS
                .iter()
                .chain(SHORTCUT_ACTION_KEYS.iter())
                .copied()
                .collect();
            warn_unknown_section_keys(&mut diags, "shortcuts", t, &known);
        }
        if let Some(t) = section(&table, "font") {
            warn_unknown_section_keys(&mut diags, "font", t, FONT_KEYS);
            for face_label in ["normal", "bold", "italic", "bold_italic", "ui"] {
                if let Some(face) = t.get(face_label).and_then(Value::as_table) {
                    let label = format!("font.{face_label}");
                    warn_unknown_section_keys(&mut diags, &label, face, FONT_FACE_KEYS);
                }
            }
        }
        if let Some(t) = section(&table, "mouse") {
            warn_unknown_section_keys(&mut diags, "mouse", t, MOUSE_KEYS);
        }
        if let Some(t) = section(&table, "keyboard") {
            warn_unknown_section_keys(&mut diags, "keyboard", t, KEYBOARD_KEYS);
        }
        if let Some(t) = section(&table, "inactive_pane") {
            warn_unknown_section_keys(&mut diags, "inactive_pane", t, INACTIVE_PANE_KEYS);
        }
        if let Some(t) = section(&table, "orzma") {
            warn_unknown_section_keys(&mut diags, "orzma", t, ORZMA_KEYS);
        }
        if let Some(t) = section(&table, "scrollback") {
            warn_unknown_section_keys(&mut diags, "scrollback", t, SCROLLBACK_KEYS);
        }
        let settings = RawSettings {
            shortcuts: RawShortcuts::from_legacy_section(section(&table, "shortcuts")),
            vi_mode: RawViMode::from_legacy_section(section(&table, "vi-mode")),
            font: RawFont::from_legacy_section(section(&table, "font")),
            mouse: RawMouse::from_legacy_section(section(&table, "mouse")),
            keyboard: RawKeyboard::from_legacy_section(section(&table, "keyboard")),
            inactive_pane: RawInactivePane::from_legacy_section(section(&table, "inactive_pane")),
            orzma: RawOrzma::from_legacy_section(section(&table, "orzma")),
            scrollback: RawScrollback::from_legacy_section(section(&table, "scrollback")),
        };
        Ok((settings, diags))
    }
}

/// Pushes one `Warn` diagnostic per key in `table`'s top level that is not a
/// known section name.
fn warn_unknown_top_level_sections(diags: &mut Vec<Diagnostic>, table: &Table) {
    for key in table.keys() {
        if !TOP_LEVEL_SECTIONS.contains(&key.as_str()) {
            diags.push(Diagnostic {
                severity: Severity::Warn,
                message: format!("unknown top-level section `[{key}]`; ignored"),
            });
        }
    }
}

/// Pushes one `Warn` diagnostic per key in `table` that is not listed in
/// `known` — `table` is one section's raw TOML table and `known` is the
/// fixed set of field names that section's typed `Raw*::from_legacy_section`
/// actually reads.
fn warn_unknown_section_keys(
    diags: &mut Vec<Diagnostic>,
    section_label: &str,
    table: &Table,
    known: &[&str],
) {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            diags.push(Diagnostic {
                severity: Severity::Warn,
                message: format!("unknown `[{section_label}]` key `{key}`; ignored"),
            });
        }
    }
}

impl RawShortcuts {
    /// Builds `RawShortcuts` from a legacy `[shortcuts]` sub-table: every key
    /// other than `leader` / `leader-tap-timeout-ms` / `repeat-time-ms` is
    /// treated as an action binding and copied into `bindings` verbatim
    /// (legacy action keys are already kebab-case, matching the new
    /// `bindings` map's key shape) — only if the user actually set it. The
    /// two timeout scalars were kebab-case in the legacy schema and are
    /// mapped here to their new snake_case fields; `leader` needs no
    /// renaming. A value of the wrong TOML type for its key is skipped
    /// (left at the built-in default) rather than treated as a hard error.
    fn from_legacy_section(section: Option<&Table>) -> Self {
        let mut raw = RawShortcuts::default();
        let Some(section) = section else {
            return raw;
        };
        for (key, value) in section {
            match key.as_str() {
                "leader" => raw.leader = value.as_str().map(str::to_string),
                "leader-tap-timeout-ms" => {
                    if let Some(ms) = as_u64(value) {
                        raw.leader_tap_timeout_ms = ms;
                    }
                }
                "repeat-time-ms" => {
                    if let Some(ms) = as_u64(value) {
                        raw.repeat_time_ms = ms;
                    }
                }
                action => {
                    if let Some(chord) = value.as_str() {
                        raw.bindings.insert(action.to_string(), chord.to_string());
                    }
                }
            }
        }
        raw
    }
}

impl RawViMode {
    /// Builds `RawViMode` from a legacy `[vi-mode]` sub-table: every present
    /// action's value is normalized to a `Vec<String>` — a bare string
    /// becomes a one-element array, an array is copied through unchanged.
    /// Actions absent from the legacy file are left out of `bindings`
    /// entirely (never filled in), matching the same presence-only rule as
    /// `RawShortcuts::from_legacy_section`.
    fn from_legacy_section(section: Option<&Table>) -> Self {
        let mut raw = RawViMode::default();
        let Some(section) = section else {
            return raw;
        };
        for (action, value) in section {
            let keys = match value {
                Value::String(s) => vec![s.clone()],
                Value::Array(items) => items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                _ => continue,
            };
            raw.bindings.insert(action.clone(), keys);
        }
        raw
    }
}

impl RawFont {
    /// Builds `RawFont` from a legacy `[font]` sub-table. Legacy `[font]`
    /// field names are already the new snake_case spellings (no
    /// `rename_all` was ever applied to `FontConfig`), so each field is
    /// copied straight through when present; an omitted field keeps
    /// `RawFont::default()`'s value. `ui` is included for parity with the
    /// four terminal faces, even though no legacy release ever wrote it —
    /// an omitted `[font.ui]` simply keeps the default empty face.
    fn from_legacy_section(section: Option<&Table>) -> Self {
        let default = RawFont::default();
        let Some(section) = section else {
            return default;
        };
        RawFont {
            size: as_f32(section.get("size")).unwrap_or(default.size),
            normal: RawFace::from_legacy_face(section.get("normal").and_then(Value::as_table)),
            bold: RawFace::from_legacy_face(section.get("bold").and_then(Value::as_table)),
            italic: RawFace::from_legacy_face(section.get("italic").and_then(Value::as_table)),
            bold_italic: RawFace::from_legacy_face(
                section.get("bold_italic").and_then(Value::as_table),
            ),
            ui: RawFace::from_legacy_face(section.get("ui").and_then(Value::as_table)),
        }
    }
}

impl RawFace {
    /// Builds one `RawFace` from a legacy `[font.<face>]` sub-table (or an
    /// inline `<face> = { family = "…" }` table — both parse to the same
    /// `toml::Table` shape).
    fn from_legacy_face(face: Option<&Table>) -> Self {
        let Some(face) = face else {
            return RawFace::default();
        };
        RawFace {
            family: face
                .get("family")
                .and_then(Value::as_str)
                .map(str::to_string),
            style: face
                .get("style")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }
}

impl RawMouse {
    /// Builds `RawMouse` from a legacy `[mouse]` sub-table. Legacy `[mouse]`
    /// field names are already snake_case (no `rename_all` on
    /// `MouseConfig`), so each field is copied straight through when
    /// present; `fine_modifier` stays a `String` (parsed downstream by
    /// `resolve`). An omitted field keeps `RawMouse::default()`'s value.
    fn from_legacy_section(section: Option<&Table>) -> Self {
        let default = RawMouse::default();
        let Some(section) = section else {
            return default;
        };
        RawMouse {
            lines_per_notch: as_u32(section.get("lines_per_notch"))
                .unwrap_or(default.lines_per_notch),
            fine_modifier: section
                .get("fine_modifier")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(default.fine_modifier),
            fine_lines: as_u32(section.get("fine_lines")).unwrap_or(default.fine_lines),
            max_protocol_events_per_frame: as_u32(section.get("max_protocol_events_per_frame"))
                .unwrap_or(default.max_protocol_events_per_frame),
            cells_per_notch: as_f32(section.get("cells_per_notch"))
                .unwrap_or(default.cells_per_notch),
            axis_lock_ratio: as_f32(section.get("axis_lock_ratio"))
                .unwrap_or(default.axis_lock_ratio),
            double_click_timeout_ms: as_u32(section.get("double_click_timeout_ms"))
                .unwrap_or(default.double_click_timeout_ms),
            click_drift_px: as_f32(section.get("click_drift_px")).unwrap_or(default.click_drift_px),
            autoscroll_base_period_ms: as_u32(section.get("autoscroll_base_period_ms"))
                .unwrap_or(default.autoscroll_base_period_ms),
            autoscroll_min_period_ms: as_u32(section.get("autoscroll_min_period_ms"))
                .unwrap_or(default.autoscroll_min_period_ms),
            autoscroll_step_ms: as_u32(section.get("autoscroll_step_ms"))
                .unwrap_or(default.autoscroll_step_ms),
            drag_threshold_px: as_f32(section.get("drag_threshold_px"))
                .unwrap_or(default.drag_threshold_px),
            divider_grab_tolerance_px: as_f32(section.get("divider_grab_tolerance_px"))
                .unwrap_or(default.divider_grab_tolerance_px),
        }
    }
}

impl RawKeyboard {
    /// Builds `RawKeyboard` from a legacy `[keyboard]` sub-table.
    /// `option_as_alt` stays a `String` (parsed downstream by `resolve`).
    fn from_legacy_section(section: Option<&Table>) -> Self {
        let default = RawKeyboard::default();
        let Some(section) = section else {
            return default;
        };
        RawKeyboard {
            option_as_alt: section
                .get("option_as_alt")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(default.option_as_alt),
        }
    }
}

impl RawInactivePane {
    /// Builds `RawInactivePane` from a legacy `[inactive_pane]` sub-table.
    /// Legacy field names are already snake_case (no `rename_all` on
    /// `InactivePaneConfig`), so each field is copied straight through when
    /// present.
    fn from_legacy_section(section: Option<&Table>) -> Self {
        let default = RawInactivePane::default();
        let Some(section) = section else {
            return default;
        };
        RawInactivePane {
            enabled: section
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(default.enabled),
            dim: as_f32(section.get("dim")).unwrap_or(default.dim),
            tint_color: section
                .get("tint_color")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(default.tint_color),
            tint: as_f32(section.get("tint")).unwrap_or(default.tint),
            webview_dim: as_f32(section.get("webview_dim")).unwrap_or(default.webview_dim),
            webview_desaturate: as_f32(section.get("webview_desaturate"))
                .unwrap_or(default.webview_desaturate),
        }
    }
}

impl RawOrzma {
    /// Builds `RawOrzma` from a legacy `[orzma]` sub-table.
    fn from_legacy_section(section: Option<&Table>) -> Self {
        let Some(section) = section else {
            return RawOrzma::default();
        };
        RawOrzma {
            shell: section
                .get("shell")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }
}

impl RawScrollback {
    /// Builds `RawScrollback` from a legacy `[scrollback]` sub-table. The
    /// legacy `seed-lines` key (kebab-case) is mapped to the new
    /// `seed_lines` field.
    fn from_legacy_section(section: Option<&Table>) -> Self {
        let default = RawScrollback::default();
        let Some(section) = section else {
            return default;
        };
        RawScrollback {
            seed_lines: section
                .get("seed-lines")
                .and_then(Value::as_integer)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(default.seed_lines),
        }
    }
}

/// Returns `table[name]` as a sub-table, or `None` if the key is absent or
/// not a table.
fn section<'a>(table: &'a Table, name: &str) -> Option<&'a Table> {
    table.get(name)?.as_table()
}

/// Coerces a TOML integer or float value into an `f32`.
fn as_f32(value: Option<&Value>) -> Option<f32> {
    let value = value?;
    value
        .as_float()
        .map(|f| f as f32)
        .or_else(|| value.as_integer().map(|i| i as f32))
}

/// Coerces a TOML integer value into a `u32`, discarding out-of-range or
/// negative values rather than truncating them.
fn as_u32(value: Option<&Value>) -> Option<u32> {
    u32::try_from(value?.as_integer()?).ok()
}

/// Coerces a TOML integer value into a `u64`, discarding out-of-range or
/// negative values rather than truncating them.
fn as_u64(value: &Value) -> Option<u64> {
    u64::try_from(value.as_integer()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_user_set_bindings_migrate() {
        let legacy = "[shortcuts]\nquit = \"Cmd+Shift+Q\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(
            raw.shortcuts.bindings.get("quit").map(String::as_str),
            Some("Cmd+Shift+Q")
        );
        assert!(
            !raw.shortcuts.bindings.contains_key("paste"),
            "unset actions must not be frozen as overrides"
        );
    }

    #[test]
    fn vi_mode_single_string_becomes_array() {
        let legacy = "[vi-mode]\nup = \"k\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(raw.vi_mode.bindings.get("up"), Some(&vec!["k".to_string()]));
    }

    #[test]
    fn vi_mode_array_stays_array() {
        let legacy = "[vi-mode]\ndown = [\"j\", \"ArrowDown\"]\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(
            raw.vi_mode.bindings.get("down"),
            Some(&vec!["j".to_string(), "ArrowDown".to_string()])
        );
    }

    #[test]
    fn vi_mode_unset_actions_are_not_frozen() {
        let legacy = "[vi-mode]\nup = \"k\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert!(!raw.vi_mode.bindings.contains_key("down"));
    }

    #[test]
    fn shortcuts_kebab_timeout_keys_map_to_snake_case_fields() {
        let legacy = "[shortcuts]\nleader-tap-timeout-ms = 900\nrepeat-time-ms = 42\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(raw.shortcuts.leader_tap_timeout_ms, 900);
        assert_eq!(raw.shortcuts.repeat_time_ms, 42);
    }

    #[test]
    fn shortcuts_leader_copies_through_unchanged() {
        let legacy = "[shortcuts]\nleader = \"Ctrl+A\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(raw.shortcuts.leader.as_deref(), Some("Ctrl+A"));
    }

    #[test]
    fn shortcuts_empty_string_unbind_is_preserved() {
        let legacy = "[shortcuts]\nquit = \"\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(
            raw.shortcuts.bindings.get("quit").map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn scrollback_seed_lines_kebab_maps_to_snake_case() {
        let legacy = "[scrollback]\nseed-lines = 5000\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(raw.scrollback.seed_lines, 5000);
    }

    #[test]
    fn scrollback_omitted_keeps_default() {
        let (raw, _diags) = RawSettings::from_legacy_toml("").expect("valid legacy toml");
        assert_eq!(
            raw.scrollback.seed_lines,
            RawScrollback::default().seed_lines
        );
    }

    #[test]
    fn font_section_copies_present_fields_keeps_omitted_defaults() {
        let legacy = "[font]\nsize = 14.0\n[font.normal]\nfamily = \"Iosevka\"\nstyle = \"Bold\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(raw.font.size, 14.0);
        assert_eq!(raw.font.normal.family.as_deref(), Some("Iosevka"));
        assert_eq!(raw.font.normal.style.as_deref(), Some("Bold"));
        assert_eq!(raw.font.bold, RawFace::default());
        assert_eq!(raw.font.italic, RawFace::default());
    }

    #[test]
    fn font_inline_face_table_is_accepted() {
        let legacy = "[font]\nnormal = { family = \"Iosevka\" }\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(raw.font.normal.family.as_deref(), Some("Iosevka"));
    }

    #[test]
    fn mouse_section_copies_present_fields_keeps_omitted_defaults() {
        let legacy = "[mouse]\nlines_per_notch = 5\nfine_modifier = \"ctrl\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(raw.mouse.lines_per_notch, 5);
        assert_eq!(raw.mouse.fine_modifier, "ctrl");
        assert_eq!(raw.mouse.fine_lines, RawMouse::default().fine_lines);
    }

    #[test]
    fn keyboard_option_as_alt_copied_as_string() {
        let legacy = "[keyboard]\noption_as_alt = \"both\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(raw.keyboard.option_as_alt, "both");
    }

    #[test]
    fn inactive_pane_section_copies_present_fields() {
        let legacy = "[inactive_pane]\nenabled = false\ntint = 0.3\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert!(!raw.inactive_pane.enabled);
        assert_eq!(raw.inactive_pane.tint, 0.3);
        assert_eq!(raw.inactive_pane.dim, RawInactivePane::default().dim);
    }

    #[test]
    fn orzma_shell_copied_when_present() {
        let legacy = "[orzma]\nshell = \"/usr/bin/fish\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(raw.orzma.shell.as_deref(), Some("/usr/bin/fish"));
    }

    #[test]
    fn empty_legacy_config_yields_full_defaults() {
        let (raw, _diags) =
            RawSettings::from_legacy_toml("").expect("empty text is valid (empty) toml");
        assert_eq!(raw, RawSettings::default());
    }

    #[test]
    fn malformed_toml_is_a_parse_error() {
        let err = RawSettings::from_legacy_toml("this is not [ valid toml")
            .expect_err("malformed TOML must not silently fall back to defaults");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn valid_minimal_toml_is_ok() {
        let (raw, _diags) = RawSettings::from_legacy_toml("[shortcuts]\n")
            .expect("a valid-but-minimal document must parse, even though it sets nothing");
        assert_eq!(raw, RawSettings::default());
    }

    #[test]
    fn unknown_top_level_section_does_not_panic_or_fail() {
        let legacy = "[not-a-real-section]\nfoo = \"bar\"\n[shortcuts]\nquit = \"Cmd+Q\"\n";
        let (raw, diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert_eq!(
            raw.shortcuts.bindings.get("quit").map(String::as_str),
            Some("Cmd+Q")
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("not-a-real-section")),
            "an unknown top-level section must warn, not silently vanish: {diags:?}"
        );
    }

    #[test]
    fn wrong_typed_binding_value_is_skipped_not_fatal() {
        let legacy = "[shortcuts]\nquit = 5\npaste = \"Cmd+Shift+V\"\n";
        let (raw, _diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert!(!raw.shortcuts.bindings.contains_key("quit"));
        assert_eq!(
            raw.shortcuts.bindings.get("paste").map(String::as_str),
            Some("Cmd+Shift+V")
        );
    }

    #[test]
    fn unknown_mouse_key_and_misspelled_section_both_warn() {
        let legacy = "[mouse]\nlines_pernotch = 10\n[shortucts]\nquit = \"Cmd+Q\"\n";
        let (raw, diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert!(
            diags.iter().any(|d| d.message.contains("lines_pernotch")),
            "a misspelled [mouse] key must warn: {diags:?}"
        );
        assert!(
            diags.iter().any(|d| d.message.contains("shortucts")),
            "a misspelled top-level section must warn: {diags:?}"
        );
        assert_eq!(
            raw.mouse.lines_per_notch,
            RawMouse::default().lines_per_notch,
            "the typo'd key must be ignored, not crash or corrupt the rest of [mouse]"
        );
    }

    #[test]
    fn unknown_font_face_key_warns() {
        let legacy = "[font.normal]\nfamilly = \"Iosevka\"\n";
        let (_raw, diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("familly") && d.message.contains("font.normal")),
            "a misspelled [font.normal] key must warn: {diags:?}"
        );
    }

    #[test]
    fn known_keys_do_not_warn() {
        let legacy = "[shortcuts]\nleader = \"Ctrl+A\"\nquit = \"Cmd+Q\"\n[font]\nsize = 14.0\n\
             [font.normal]\nfamily = \"Iosevka\"\n[mouse]\nlines_per_notch = 5\n";
        let (_raw, diags) = RawSettings::from_legacy_toml(legacy).expect("valid legacy toml");
        assert!(diags.is_empty(), "no known key should ever warn: {diags:?}");
    }
}
