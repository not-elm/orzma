//! Resolves a `RawSettings` into a fully-typed `OrzmaConfigs`, collecting
//! per-item diagnostics instead of failing on the first error.

use crate::OrzmaConfigs;
use crate::raw::RawSettings;
use crate::shortcuts::{SHORTCUT_ACTION_KEYS, Shortcuts, parse_binding, parse_leader};
use toml::Value;
use toml::value::Table;

/// Severity of a resolution diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Non-fatal: the offending entry was skipped; everything else resolved.
    Warn,
    /// A resolution step failed outright and a default was substituted.
    Error,
}

/// One resolution diagnostic (warn-and-continue; never fatal here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// How serious the diagnostic is.
    pub severity: Severity,
    /// Human-readable description, naming the offending key/value.
    pub message: String,
}

impl RawSettings {
    /// Resolves raw settings into typed `OrzmaConfigs`, returning diagnostics
    /// collected along the way instead of failing on the first bad entry.
    pub fn resolve(&self) -> (OrzmaConfigs, Vec<Diagnostic>) {
        let mut diags = Vec::new();
        let shortcuts = self.resolve_shortcuts(&mut diags);
        // NOTE: vi-mode, font, and the remaining scalar sections are wired in
        // a later task; until then every other field keeps `OrzmaConfigs`'s
        // built-in default.
        let cfg = OrzmaConfigs {
            shortcuts,
            ..OrzmaConfigs::default()
        };
        (cfg, diags)
    }

    /// Re-emits `self.shortcuts` as a `toml::Table` and feeds it through the
    /// existing `Shortcuts` deserializer, which already does the action
    /// routing, `""`-unbind handling, and per-field defaulting.
    fn resolve_shortcuts(&self, diags: &mut Vec<Diagnostic>) -> Shortcuts {
        let mut table = Table::new();
        if let Some(leader) = &self.shortcuts.leader {
            match parse_leader(leader) {
                Ok(_) => {
                    table.insert("leader".into(), Value::String(leader.clone()));
                }
                Err(e) => diags.push(Diagnostic {
                    severity: Severity::Warn,
                    message: format!("leader `{leader}`: {e}; using built-in default leader"),
                }),
            }
        }
        for (action, chord) in &self.shortcuts.bindings {
            if !SHORTCUT_ACTION_KEYS.contains(&action.as_str()) {
                diags.push(Diagnostic {
                    severity: Severity::Warn,
                    message: format!("unknown shortcut action `{action}`; ignored"),
                });
                continue;
            }
            // NOTE: pre-parse each value so ONE bad chord only skips its own
            // binding (a whole-struct try_into would reject every binding at
            // once, resetting all of them to `Shortcuts::default()`). An
            // empty string is a valid unbind and passes through to the
            // deserializer untouched. The same reasoning is why `leader`
            // above is pre-validated before insertion rather than left for
            // the deserializer to reject.
            if !chord.is_empty()
                && let Err(e) = parse_binding(chord)
            {
                diags.push(Diagnostic {
                    severity: Severity::Warn,
                    message: format!("shortcut `{action}` = `{chord}`: {e}; skipped"),
                });
                continue;
            }
            table.insert(action.clone(), Value::String(chord.clone()));
        }
        // NOTE: every remaining entry (leader included) was pre-validated
        // above, so this deserialize cannot fail on a bad chord; a failure
        // here would be a bug in the pre-validation itself — surface it
        // rather than silently resetting all shortcuts to their defaults.
        let mut shortcuts: Shortcuts = match Value::Table(table).try_into() {
            Ok(s) => s,
            Err(e) => {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("shortcuts deserialize failed after pre-validation: {e}"),
                });
                Shortcuts::default()
            }
        };
        shortcuts.leader_tap_timeout_ms = self.shortcuts.leader_tap_timeout_ms;
        shortcuts.repeat_time_ms = self.shortcuts.repeat_time_ms;
        shortcuts.normalize();
        shortcuts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn user_binding_overrides_default_others_kept() {
        let mut raw = RawSettings::default();
        raw.shortcuts.bindings = BTreeMap::from([("quit".into(), "Cmd+Shift+Q".into())]);
        let (cfg, diags) = raw.resolve();
        assert!(diags.iter().all(|d| d.severity == Severity::Warn));
        let quit = cfg.shortcuts.quit.as_ref().unwrap().chord();
        assert!(quit.modifiers.meta && quit.modifiers.shift);
        // A binding the user did NOT set keeps its built-in default.
        assert!(cfg.shortcuts.paste.is_some());
    }

    #[test]
    fn empty_string_unbinds() {
        let mut raw = RawSettings::default();
        raw.shortcuts.bindings = BTreeMap::from([("quit".into(), "".into())]);
        let (cfg, _) = raw.resolve();
        assert!(cfg.shortcuts.quit.is_none());
    }

    #[test]
    fn unknown_action_key_warns_and_is_skipped() {
        let mut raw = RawSettings::default();
        raw.shortcuts.bindings = BTreeMap::from([("qiut".into(), "Cmd+Q".into())]);
        let (_cfg, diags) = raw.resolve();
        assert!(diags.iter().any(|d| d.message.contains("qiut")));
    }

    #[test]
    fn bad_leader_warns_and_keeps_bindings() {
        let mut raw = RawSettings::default();
        raw.shortcuts.leader = Some("Cmd+Foo".into());
        raw.shortcuts.bindings = BTreeMap::from([("quit".into(), "Cmd+Shift+Q".into())]);
        let (cfg, diags) = raw.resolve();
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Warn && d.message.contains("Cmd+Foo"))
        );
        // A bad leader falls back to the built-in default leader...
        assert_eq!(cfg.shortcuts.leader, Shortcuts::default().leader);
        // ...and does NOT reset the user's other bindings.
        let quit = cfg.shortcuts.quit.as_ref().unwrap().chord();
        assert!(quit.modifiers.meta && quit.modifiers.shift);
    }

    #[test]
    fn bad_chord_for_known_action_skips_only_that_binding() {
        let mut raw = RawSettings::default();
        raw.shortcuts.bindings = BTreeMap::from([
            ("quit".into(), "Cmd+Foo".into()),
            ("paste".into(), "Cmd+Shift+V".into()),
        ]);
        let (cfg, diags) = raw.resolve();
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Warn && d.message.contains("quit"))
        );
        // The bad chord is skipped; `quit` falls back to its built-in default.
        assert_eq!(cfg.shortcuts.quit, Shortcuts::default().quit);
        // `paste` still resolves to the user's override.
        let paste = cfg.shortcuts.paste.as_ref().unwrap().chord();
        assert!(paste.modifiers.meta && paste.modifiers.shift);
    }
}
