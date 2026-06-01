//! TOML-deserialization layer for `OzmuxConfigs`. Holds optional sections
//! and merges them onto a baseline `OzmuxConfigs::default()`.

use crate::OzmuxConfigs;
use crate::OzmuxConfigsError;
use crate::OzmuxConfigsResult;
use crate::browser::BrowserPatch;
use crate::font::FontPatch;
use crate::inactive_pane::InactivePaneConfigPatch;
use crate::mouse::MousePatch;
use crate::shortcuts::Shortcuts;
use crate::theme::ThemePatch;
use serde::Deserialize;

/// Top-level TOML shape: every section is optional. `deny_unknown_fields`
/// is critical — it catches misspelled section names (`[shortucts]`).
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfigs {
    pub(crate) shortcuts: Option<Shortcuts>,
    pub(crate) theme: Option<ThemePatch>,
    pub(crate) font: Option<FontPatch>,
    pub(crate) browser: Option<BrowserPatch>,
    pub(crate) mouse: Option<MousePatch>,
    pub(crate) inactive_pane: Option<InactivePaneConfigPatch>,
}

impl RawConfigs {
    /// Applies any populated fields onto `base` and returns the merged result.
    /// Within the `shortcuts` section, `bindings` is full-replace when present.
    /// The `theme`, `font`, `browser`, `mouse`, and `inactive_pane` sections use
    /// their respective `Patch::apply_to` for per-field merge.
    pub(crate) fn apply_to(self, mut base: OzmuxConfigs) -> OzmuxConfigs {
        if let Some(shortcuts) = self.shortcuts {
            base.shortcuts = shortcuts;
        }
        if let Some(patch) = self.theme {
            base.theme = patch.apply_to(base.theme);
        }
        if let Some(patch) = self.font {
            base.font = patch.apply_to(base.font);
        }
        if let Some(patch) = self.browser {
            base.browser = patch.apply_to(base.browser);
        }
        if let Some(patch) = self.mouse {
            base.mouse = patch.apply_to(base.mouse);
        }
        if let Some(patch) = self.inactive_pane {
            base.inactive_pane = patch.apply_to(base.inactive_pane);
        }
        base
    }
}

/// Cross-section validation: chord conflicts, font size range, etc.
pub(crate) fn validate(configs: &OzmuxConfigs) -> OzmuxConfigsResult {
    if let Err(dupes) = configs.shortcuts.bindings.validate_no_conflicts() {
        return Err(OzmuxConfigsError::DuplicateChords(dupes));
    }
    let size = configs.font.size;
    if !(size > 0.0 && size <= 200.0) {
        return Err(OzmuxConfigsError::InvalidFontSize { size });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_raw_returns_defaults() {
        let raw = RawConfigs::default();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(
            merged.shortcuts.bindings,
            OzmuxConfigs::default().shortcuts.bindings
        );
    }

    #[test]
    fn user_override_replaces_one_binding_keeps_others() {
        let toml_str = r#"
[shortcuts.bindings]
close-pane = "Cmd+Shift+W"
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        let close = merged.shortcuts.bindings.close_pane.as_ref().unwrap();
        assert_eq!(close.key, crate::shortcuts::Key::Char('w'));
        assert!(close.modifiers.meta && close.modifiers.shift);
        let focus_left = merged.shortcuts.bindings.focus_pane_left.as_ref().unwrap();
        assert_eq!(focus_left.key, crate::shortcuts::Key::Char('h'));
    }

    #[test]
    fn user_unbind_with_empty_string_sets_field_to_none() {
        let toml_str = r#"
[shortcuts.bindings]
enter-copy-mode = ""
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert!(merged.shortcuts.bindings.enter_copy_mode.is_none());
    }

    #[test]
    fn unknown_section_is_rejected() {
        let toml_str = r#"
[shortucts]
"#;
        let err = toml::from_str::<RawConfigs>(toml_str)
            .err()
            .expect("expected parse error for unknown section");
        let msg = err.to_string();
        assert!(
            msg.contains("shortucts") || msg.contains("unknown field"),
            "error message should mention the unknown field; got: {msg}"
        );
    }

    #[test]
    fn unknown_binding_field_is_rejected() {
        let toml_str = r#"
[shortcuts.bindings]
resize-pane-down = "Cmd+Shift+J"
"#;
        let err = toml::from_str::<RawConfigs>(toml_str)
            .err()
            .expect("expected parse error for unknown binding field");
        let msg = err.to_string();
        assert!(
            msg.contains("resize-pane-down") || msg.contains("unknown field"),
            "error message should mention the unknown field; got: {msg}"
        );
    }

    #[test]
    fn inactive_pane_section_merges_and_clamps() {
        let toml_str = r#"
[inactive_pane]
enabled = false
opacity = 2.0
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert!(!merged.inactive_pane.enabled);
        assert_eq!(merged.inactive_pane.opacity, 1.0, "opacity clamps to 1.0");
        assert_eq!(merged.inactive_pane.color, "#000000", "color keeps default");
    }

    #[test]
    fn missing_inactive_pane_section_uses_defaults() {
        let raw: RawConfigs = toml::from_str("").unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(
            merged.inactive_pane,
            crate::inactive_pane::InactivePaneConfig::default()
        );
    }

    #[test]
    fn validate_detects_chord_conflict() {
        let toml_str = r#"
[shortcuts.bindings]
close-pane = "Cmd+J"
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        let err = validate(&merged).unwrap_err();
        match err {
            OzmuxConfigsError::DuplicateChords(dupes) => {
                assert_eq!(dupes.len(), 1);
                assert!(dupes[0].actions.contains(&"close-pane"));
                assert!(dupes[0].actions.contains(&"focus-pane-down"));
            }
            _ => panic!("expected DuplicateChords, got {err:?}"),
        }
    }
}
