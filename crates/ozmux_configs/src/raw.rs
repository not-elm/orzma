//! TOML-deserialization layer for `OzmuxConfigs`. Holds optional sections
//! and merges them onto a baseline `OzmuxConfigs::default()`.

use crate::OzmuxConfigs;
use crate::font::FontPatch;
use crate::inactive_pane::InactivePaneConfigPatch;
use crate::keyboard::KeyboardConfig;
use crate::mouse::MouseConfig;
use crate::osc_webview::OscWebviewConfig;
use crate::ozma::OzmaConfig;
use crate::shortcuts::Shortcuts;
use crate::startup::StartupMode;
use crate::theme::Theme;
use crate::tmux::TmuxPatch;
use serde::Deserialize;

/// Top-level TOML shape: every section is optional. `deny_unknown_fields`
/// is critical — it catches misspelled section names (`[shortucts]`).
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfigs {
    pub(crate) shortcuts: Option<Shortcuts>,
    pub(crate) theme: Option<Theme>,
    pub(crate) font: Option<FontPatch>,
    pub(crate) mouse: Option<MouseConfig>,
    pub(crate) keyboard: Option<KeyboardConfig>,
    pub(crate) inactive_pane: Option<InactivePaneConfigPatch>,
    pub(crate) osc_webview: Option<OscWebviewConfig>,
    pub(crate) tmux: Option<TmuxPatch>,
    pub(crate) ozma: Option<OzmaConfig>,
    pub(crate) startup_mode: Option<StartupMode>,
}

impl RawConfigs {
    /// Applies any populated fields onto `base` and returns the merged result.
    /// Within the `shortcuts` section, `bindings` is full-replace when present.
    /// The `font` and `inactive_pane` sections use their respective
    /// `Patch::apply_to` for per-field merge. The `theme`, `mouse`, `keyboard`,
    /// and `osc_webview` sections are full-replace (serde default fills missing
    /// fields).
    pub(crate) fn apply_to(self, mut base: OzmuxConfigs) -> OzmuxConfigs {
        if let Some(shortcuts) = self.shortcuts {
            base.shortcuts = shortcuts;
        }
        if let Some(theme) = self.theme {
            base.theme = theme;
        }
        if let Some(patch) = self.font {
            base.font = patch.apply_to(base.font);
        }
        if let Some(mouse) = self.mouse {
            base.mouse = mouse;
        }
        if let Some(keyboard) = self.keyboard {
            base.keyboard = keyboard;
        }
        if let Some(patch) = self.inactive_pane {
            base.inactive_pane = patch.apply_to(base.inactive_pane);
        }
        if let Some(osc_webview) = self.osc_webview {
            base.osc_webview = osc_webview;
        }
        if let Some(patch) = self.tmux {
            base.tmux = patch.apply_to(base.tmux);
        }
        if let Some(ozma) = self.ozma {
            base.ozma = ozma;
        }
        if let Some(m) = self.startup_mode {
            base.startup_mode = m;
        }
        base
    }
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
        assert!(
            merged.shortcuts.bindings.focus_pane_left.is_none(),
            "unspecified deprecated bindings stay None",
        );
    }

    #[test]
    fn user_unbind_with_empty_string_sets_field_to_none() {
        let toml_str = r#"
[shortcuts.bindings]
close-pane = ""
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert!(merged.shortcuts.bindings.close_pane.is_none());
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
        let toml_str = r##"
[inactive_pane]
enabled = false
tint = 2.0
tint_color = "#102030"
"##;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert!(!merged.inactive_pane.enabled);
        assert_eq!(merged.inactive_pane.tint, 1.0, "tint clamps to 1.0");
        assert_eq!(merged.inactive_pane.tint_color, "#102030");
        assert_eq!(merged.inactive_pane.dim, 1.0, "dim keeps default");
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
    fn osc_webview_setting_merges_from_toml() {
        let toml_str = r#"
[osc_webview]
enabled = false
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert!(
            !merged.osc_webview.enabled,
            "[osc_webview] enabled = false must merge through the real Raw->apply_to path, flipping the default-on gate off"
        );

        let empty: RawConfigs = toml::from_str("").unwrap();
        let defaulted = empty.apply_to(OzmuxConfigs::default());
        assert!(
            defaulted.osc_webview.enabled,
            "empty TOML must keep the default-on gate"
        );
    }

    #[test]
    fn tmux_section_merges_from_toml() {
        let toml_str = r#"
[tmux]
program = "/usr/local/bin/tmux"
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.tmux.program, "/usr/local/bin/tmux");
        assert_eq!(merged.tmux.socket_name, None);
    }

    #[test]
    fn missing_tmux_section_uses_defaults() {
        let raw: RawConfigs = toml::from_str("").unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.tmux, crate::tmux::TmuxConfig::default());
    }

    #[test]
    fn startup_mode_defaults_to_ozma() {
        let raw: RawConfigs = toml::from_str("").unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.startup_mode, crate::startup::StartupMode::Ozma);
    }

    #[test]
    fn startup_mode_auto_attach_parses() {
        let raw: RawConfigs = toml::from_str(r#"startup_mode = "auto-attach""#).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.startup_mode, crate::startup::StartupMode::AutoAttach);
    }

    #[test]
    fn startup_mode_ozmux_parses() {
        let raw: RawConfigs = toml::from_str(r#"startup_mode = "ozmux""#).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.startup_mode, crate::startup::StartupMode::Ozmux);
    }

    #[test]
    fn unknown_startup_mode_is_rejected() {
        assert!(toml::from_str::<RawConfigs>(r#"startup_mode = "invalid""#).is_err());
    }

    #[test]
    fn keyboard_section_merges_from_toml() {
        let toml_str = r#"
[keyboard]
option_as_alt = "both"
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(
            merged.keyboard.option_as_alt,
            crate::keyboard::OptionAsAlt::Both
        );
    }

    #[test]
    fn missing_keyboard_section_uses_defaults() {
        let raw: RawConfigs = toml::from_str("").unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.keyboard, crate::keyboard::KeyboardConfig::default());
    }

    #[test]
    fn ozma_section_parses_from_toml() {
        let toml_str = r#"
[ozma]
shell = "/usr/bin/zsh"
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.ozma.shell.as_deref(), Some("/usr/bin/zsh"));
    }

    #[test]
    fn missing_ozma_section_uses_defaults() {
        let raw: RawConfigs = toml::from_str("").unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert!(merged.ozma.shell.is_none());
    }
}
