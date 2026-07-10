//! Resolves a `RawSettings` into a fully-typed `OrzmaConfigs`, collecting
//! per-item diagnostics instead of failing on the first error.

use crate::OrzmaConfigs;
use crate::font::{FontConfig, FontFaceConfig, FontStyleSpec};
use crate::inactive_pane::InactivePaneConfig;
use crate::keyboard::{KeyboardConfig, OptionAsAlt};
use crate::mouse::{FineModifier, MouseConfig};
use crate::orzma::OrzmaConfig;
use crate::raw::RawSettings;
use crate::scrollback::ScrollbackConfig;
use crate::shortcuts::{Leader, SHORTCUT_ACTION_KEYS, Shortcuts, parse_binding, parse_leader};
use crate::vi_mode::{VI_MODE_ACTION_KEYS, ViModeConfig, ViModeKey, parse_vi_mode_key};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
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
        let vi_mode = self.resolve_vi_mode(&mut diags);
        let font = self.resolve_font(&mut diags);
        let mouse = self.resolve_mouse(&mut diags);
        let keyboard = self.resolve_keyboard(&mut diags);
        let inactive_pane = self.resolve_inactive_pane();
        let orzma = self.resolve_orzma();
        let scrollback = self.resolve_scrollback();
        let cfg = OrzmaConfigs {
            shortcuts,
            vi_mode,
            font,
            mouse,
            keyboard,
            inactive_pane,
            orzma,
            scrollback,
        };
        (cfg, diags)
    }

    /// Re-emits `self.shortcuts` as a `toml::Table` and feeds it through the
    /// existing `Shortcuts` deserializer, which already does the action
    /// routing, `""`-unbind handling, and per-field defaulting. Then repairs
    /// any chord collision or leader problem the merged result exposes (see
    /// `conflicting_shortcut_actions` / `leader_needs_revert`) by dropping
    /// the losing binding instead of failing the whole config.
    fn resolve_shortcuts(&self, diags: &mut Vec<Diagnostic>) -> Shortcuts {
        let mut table = Table::new();
        self.collect_shortcut_bindings(&mut table, diags);
        let shortcuts = self.build_shortcuts(diags, &table);

        let losers = Self::conflicting_shortcut_actions(diags, &shortcuts);
        let revert_leader = Self::leader_needs_revert(diags, &shortcuts);
        if losers.is_empty() && !revert_leader {
            return shortcuts;
        }
        for action in losers {
            table.insert(action.to_string(), Value::String(String::new()));
        }
        if revert_leader {
            table.remove("leader");
        }
        self.build_shortcuts(diags, &table)
    }

    /// Pre-validates `self.shortcuts.leader` and `self.shortcuts.bindings`
    /// entry-by-entry, inserting only the entries that parse into `table` so
    /// one bad chord only skips its own binding (a whole-struct `try_into`
    /// would reject every binding at once, resetting all of them to
    /// `Shortcuts::default()`). An empty string is a valid unbind — for
    /// `leader` it means "disabled" — and passes through to the deserializer
    /// untouched, without going through `parse_leader` (which rejects the
    /// empty string as a parse error, not a disable request).
    fn collect_shortcut_bindings(&self, table: &mut Table, diags: &mut Vec<Diagnostic>) {
        if let Some(leader) = &self.shortcuts.leader {
            if leader.is_empty() {
                table.insert("leader".into(), Value::String(String::new()));
            } else {
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
        }
        for (action, chord) in &self.shortcuts.bindings {
            if !SHORTCUT_ACTION_KEYS.contains(&action.as_str()) {
                diags.push(Diagnostic {
                    severity: Severity::Warn,
                    message: format!("unknown shortcut action `{action}`; ignored"),
                });
                continue;
            }
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
    }

    /// Deserializes `table` into a `Shortcuts`, applies the two timeout
    /// scalars, and normalizes. `table` is expected to already be
    /// pre-validated (see `collect_shortcut_bindings`), so a deserialize
    /// failure here would be a bug in that pre-validation rather than a bad
    /// user value — it is surfaced as an `Error` diagnostic rather than
    /// silently resetting all shortcuts to their defaults.
    fn build_shortcuts(&self, diags: &mut Vec<Diagnostic>, table: &Table) -> Shortcuts {
        let mut shortcuts: Shortcuts = match Value::Table(table.clone()).try_into() {
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

    /// Re-emits `self.vi_mode.bindings` as a `toml::Table` and feeds it
    /// through the `ViModeConfig` deserializer, mirroring
    /// `collect_shortcut_bindings` / `build_shortcuts`: each action's key
    /// array is pre-validated as a whole (mirrors `resolve_shortcuts`'s
    /// per-entry granularity — one action is one entry), so a bad key in one
    /// action's array skips only that action, not the whole `[vi-mode]`
    /// table. Any key collision the merged result exposes is then repaired
    /// by dropping the key from every losing action but the first (in fixed
    /// `VI_MODE_ACTION_KEYS` order).
    fn resolve_vi_mode(&self, diags: &mut Vec<Diagnostic>) -> ViModeConfig {
        let mut table = Table::new();
        for (action, keys) in &self.vi_mode.bindings {
            if !VI_MODE_ACTION_KEYS.contains(&action.as_str()) {
                diags.push(Diagnostic {
                    severity: Severity::Warn,
                    message: format!("unknown vi-mode action `{action}`; ignored"),
                });
                continue;
            }
            match keys
                .iter()
                .find_map(|k| parse_vi_mode_key(k).err().map(|e| (k, e)))
            {
                Some((bad_key, e)) => diags.push(Diagnostic {
                    severity: Severity::Warn,
                    message: format!("vi-mode `{action}` key `{bad_key}`: {e}; action skipped"),
                }),
                None => {
                    let values = keys.iter().cloned().map(Value::String).collect();
                    table.insert(action.clone(), Value::Array(values));
                }
            }
        }
        let vi_mode: ViModeConfig = match Value::Table(table.clone()).try_into() {
            Ok(v) => v,
            Err(e) => {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("vi-mode deserialize failed after pre-validation: {e}"),
                });
                return ViModeConfig::default();
            }
        };

        let Err(dupes) = vi_mode.validate_no_duplicate_keys() else {
            return vi_mode;
        };
        let mut remaining: BTreeMap<&'static str, Vec<ViModeKey>> = vi_mode
            .bindings_iter()
            .map(|(label, keys, _)| (label, keys.clone()))
            .collect();
        let mut touched = BTreeSet::new();
        for dupe in &dupes {
            let winner = dupe.actions[0];
            for &loser in &dupe.actions[1..] {
                diags.push(Diagnostic {
                    severity: Severity::Warn,
                    message: format!(
                        "duplicate vi-mode key `{}` is bound to both `{winner}` and `{loser}`; keeping `{winner}`",
                        dupe.key
                    ),
                });
                if let Some(keys) = remaining.get_mut(loser) {
                    keys.retain(|k| k != &dupe.key);
                }
                touched.insert(loser);
            }
        }
        for label in touched {
            let values = remaining
                .get(label)
                .into_iter()
                .flatten()
                .map(|k| Value::String(k.to_string()))
                .collect();
            table.insert(label.to_string(), Value::Array(values));
        }
        match Value::Table(table).try_into() {
            Ok(v) => v,
            Err(e) => {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    message: format!("vi-mode deserialize failed after de-duplication: {e}"),
                });
                ViModeConfig::default()
            }
        }
    }

    /// Copies `self.font` into a `FontConfig`: `size` is clamped to
    /// `(0, 200]` (warning on out-of-range), and each face's `style` is
    /// sanitized to `None` with a warning when it fails to parse — a bad
    /// style must not reach a downstream `src/font` `.expect()`.
    fn resolve_font(&self, diags: &mut Vec<Diagnostic>) -> FontConfig {
        let default_size = FontConfig::default().size;
        let size = if self.font.size > 0.0 && self.font.size <= 200.0 {
            self.font.size
        } else {
            diags.push(Diagnostic {
                severity: Severity::Warn,
                message: format!(
                    "font size {} is out of range (expected 0 < size <= 200); using default {default_size}",
                    self.font.size
                ),
            });
            default_size
        };
        FontConfig {
            size,
            normal: Self::resolve_font_face(
                diags,
                "normal",
                &self.font.normal.family,
                &self.font.normal.style,
            ),
            bold: Self::resolve_font_face(
                diags,
                "bold",
                &self.font.bold.family,
                &self.font.bold.style,
            ),
            italic: Self::resolve_font_face(
                diags,
                "italic",
                &self.font.italic.family,
                &self.font.italic.style,
            ),
            bold_italic: Self::resolve_font_face(
                diags,
                "bold_italic",
                &self.font.bold_italic.family,
                &self.font.bold_italic.style,
            ),
        }
    }

    /// Field-by-field copy of `self.mouse` into a `MouseConfig`, parsing
    /// `fine_modifier` from its lowercase string (warning and keeping the
    /// domain default on a bad value), then running `MouseConfig::normalize`
    /// so its clamps still apply.
    fn resolve_mouse(&self, diags: &mut Vec<Diagnostic>) -> MouseConfig {
        let default = MouseConfig::default();
        let fine_modifier: FineModifier =
            match Value::String(self.mouse.fine_modifier.clone()).try_into() {
                Ok(m) => m,
                Err(_) => {
                    diags.push(Diagnostic {
                        severity: Severity::Warn,
                        message: format!(
                            "mouse.fine_modifier `{}` is not a valid value; using the default",
                            self.mouse.fine_modifier
                        ),
                    });
                    default.fine_modifier
                }
            };
        let mut mouse = MouseConfig {
            lines_per_notch: self.mouse.lines_per_notch,
            fine_modifier,
            fine_lines: self.mouse.fine_lines,
            max_protocol_events_per_frame: self.mouse.max_protocol_events_per_frame,
            cells_per_notch: self.mouse.cells_per_notch,
            axis_lock_ratio: self.mouse.axis_lock_ratio,
            double_click_timeout_ms: self.mouse.double_click_timeout_ms,
            click_drift_px: self.mouse.click_drift_px,
            autoscroll_base_period_ms: self.mouse.autoscroll_base_period_ms,
            autoscroll_min_period_ms: self.mouse.autoscroll_min_period_ms,
            autoscroll_step_ms: self.mouse.autoscroll_step_ms,
            drag_threshold_px: self.mouse.drag_threshold_px,
            divider_grab_tolerance_px: self.mouse.divider_grab_tolerance_px,
        };
        mouse.normalize();
        mouse
    }

    /// Parses `self.keyboard.option_as_alt` from its lowercase string,
    /// warning and keeping the domain default on a bad value.
    fn resolve_keyboard(&self, diags: &mut Vec<Diagnostic>) -> KeyboardConfig {
        let default = KeyboardConfig::default();
        let option_as_alt: OptionAsAlt =
            match Value::String(self.keyboard.option_as_alt.clone()).try_into() {
                Ok(v) => v,
                Err(_) => {
                    diags.push(Diagnostic {
                        severity: Severity::Warn,
                        message: format!(
                            "keyboard.option_as_alt `{}` is not a valid value; using the default",
                            self.keyboard.option_as_alt
                        ),
                    });
                    default.option_as_alt
                }
            };
        KeyboardConfig { option_as_alt }
    }

    /// Field-by-field copy of `self.inactive_pane` into an
    /// `InactivePaneConfig`, then runs `InactivePaneConfig::normalize` so its
    /// clamps still apply.
    fn resolve_inactive_pane(&self) -> InactivePaneConfig {
        let mut inactive_pane = InactivePaneConfig {
            enabled: self.inactive_pane.enabled,
            dim: self.inactive_pane.dim,
            tint_color: self.inactive_pane.tint_color.clone(),
            tint: self.inactive_pane.tint,
            webview_dim: self.inactive_pane.webview_dim,
            webview_desaturate: self.inactive_pane.webview_desaturate,
        };
        inactive_pane.normalize();
        inactive_pane
    }

    /// Copies `self.orzma` into an `OrzmaConfig`.
    fn resolve_orzma(&self) -> OrzmaConfig {
        OrzmaConfig {
            shell: self.orzma.shell.clone(),
        }
    }

    /// Copies `self.scrollback` into a `ScrollbackConfig`.
    fn resolve_scrollback(&self) -> ScrollbackConfig {
        ScrollbackConfig {
            seed_lines: self.scrollback.seed_lines,
        }
    }

    /// Detects chord collisions among the merged `shortcuts`' direct and
    /// leader-scoped bindings, plus a leader chord that shadows a direct
    /// binding, pushing a `Warn` diagnostic for each and returning the
    /// losing action labels to unbind. The first action in fixed
    /// (`bindings_iter`) order wins each collision.
    fn conflicting_shortcut_actions(
        diags: &mut Vec<Diagnostic>,
        shortcuts: &Shortcuts,
    ) -> Vec<&'static str> {
        let mut losers = Vec::new();
        for conflicts in [
            shortcuts.validate_no_direct_conflicts(),
            shortcuts.validate_no_leader_conflicts(),
        ] {
            let Err(dupes) = conflicts else { continue };
            for dupe in dupes {
                let winner = dupe.actions[0];
                for &loser in &dupe.actions[1..] {
                    diags.push(Diagnostic {
                        severity: Severity::Warn,
                        message: format!(
                            "duplicate chord {} bound to both `{winner}` and `{loser}`; keeping `{winner}`",
                            dupe.chord
                        ),
                    });
                    losers.push(loser);
                }
            }
        }
        if let Some(Leader::Chord(leader_chord)) = shortcuts.leader.as_ref()
            && let Some((action, _, _)) = shortcuts
                .direct_chords()
                .find(|(_, chord, _)| *chord == leader_chord)
        {
            diags.push(Diagnostic {
                severity: Severity::Warn,
                message: format!(
                    "leader chord {leader_chord} shadows the direct binding for `{action}`; unbinding `{action}`"
                ),
            });
            losers.push(action);
        }
        losers
    }

    /// True when the merged `shortcuts`' leader is a chord whose key has no
    /// physical mapping, meaning every `<Leader>`-scoped binding would be
    /// unreachable. Pushes a `Warn` diagnostic when true.
    fn leader_needs_revert(diags: &mut Vec<Diagnostic>, shortcuts: &Shortcuts) -> bool {
        if let Some(Leader::Chord(chord)) = shortcuts.leader.as_ref()
            && !chord.key.maps_to_physical_key()
        {
            diags.push(Diagnostic {
                severity: Severity::Warn,
                message: format!(
                    "leader chord {chord} has no physical key mapping; its <Leader> bindings would be unreachable; using the built-in default leader"
                ),
            });
            return true;
        }
        false
    }

    /// Builds one resolved `FontFaceConfig`: `family` is copied verbatim;
    /// `style` is sanitized to `None` (with a `Warn` diagnostic) when it does
    /// not parse as a `FontStyleSpec`.
    fn resolve_font_face(
        diags: &mut Vec<Diagnostic>,
        label: &'static str,
        family: &Option<String>,
        style: &Option<String>,
    ) -> FontFaceConfig {
        let style = match style {
            None => None,
            Some(s) => match FontStyleSpec::from_str(s) {
                Ok(_) => Some(s.clone()),
                Err(_) => {
                    diags.push(Diagnostic {
                        severity: Severity::Warn,
                        message: format!("font.{label}.style `{s}` is not a valid style; ignored"),
                    });
                    None
                }
            },
        };
        FontFaceConfig {
            family: family.clone(),
            style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::{Key, parse_key_chord};
    use std::collections::BTreeMap;

    #[test]
    fn user_binding_overrides_default_others_kept() {
        let mut raw = RawSettings::default();
        raw.shortcuts.bindings = BTreeMap::from([("quit".into(), "Cmd+Shift+Q".into())]);
        let (cfg, diags) = raw.resolve();
        assert!(diags.iter().all(|d| d.severity == Severity::Warn));
        let quit = cfg.shortcuts.quit.as_ref().unwrap().chord();
        assert!(quit.modifiers.meta && quit.modifiers.shift);
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
        assert_eq!(cfg.shortcuts.leader, Shortcuts::default().leader);
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
        assert_eq!(cfg.shortcuts.quit, Shortcuts::default().quit);
        let paste = cfg.shortcuts.paste.as_ref().unwrap().chord();
        assert!(paste.modifiers.meta && paste.modifiers.shift);
    }

    // NOTE: the brief's draft for this test used action key "up" and key
    // string "up", neither of which exists in the real schema (the field is
    // `cursor-up`, and a bare "up" is not a valid vi-mode key token — only
    // "ArrowUp" is). Adapted to the real action key and valid key tokens.
    #[test]
    fn vi_mode_array_values_resolve() {
        let mut raw = RawSettings::default();
        raw.vi_mode.bindings =
            BTreeMap::from([("cursor-up".into(), vec!["z".into(), "Ctrl+9".into()])]);
        let (cfg, diags) = raw.resolve();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            cfg.vi_mode.cursor_up,
            vec![
                parse_vi_mode_key("z").unwrap(),
                parse_vi_mode_key("Ctrl+9").unwrap(),
            ]
        );
        assert_eq!(cfg.vi_mode.cursor_down, ViModeConfig::default().cursor_down);
    }

    #[test]
    fn invalid_font_style_warns_and_sanitizes_to_none() {
        let mut raw = RawSettings::default();
        raw.font.bold.family = Some("Iosevka".into());
        raw.font.bold.style = Some("Blod".into());
        let (cfg, diags) = raw.resolve();
        assert!(diags.iter().any(|d| d.message.contains("Blod")));
        assert_eq!(
            cfg.font.bold.style, None,
            "invalid style must be sanitized to None"
        );
        assert_eq!(cfg.font.bold.family.as_deref(), Some("Iosevka"));
    }

    #[test]
    fn out_of_range_font_size_warns_and_clamps() {
        let mut raw = RawSettings::default();
        raw.font.size = 0.0;
        let (cfg, diags) = raw.resolve();
        assert!(diags.iter().any(|d| d.message.contains("size")));
        assert!(cfg.font.size > 0.0 && cfg.font.size <= 200.0);
    }

    #[test]
    fn keyboard_enum_parses_from_lowercase() {
        let mut raw = RawSettings::default();
        raw.keyboard.option_as_alt = "both".into();
        let (cfg, diags) = raw.resolve();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(cfg.keyboard.option_as_alt, OptionAsAlt::Both);
    }

    #[test]
    fn keyboard_enum_parse_failure_warns_and_keeps_default() {
        let mut raw = RawSettings::default();
        raw.keyboard.option_as_alt = "sideways".into();
        let (cfg, diags) = raw.resolve();
        assert!(diags.iter().any(|d| d.message.contains("sideways")));
        assert_eq!(cfg.keyboard.option_as_alt, OptionAsAlt::default());
    }

    #[test]
    fn mouse_fine_modifier_parses_from_lowercase() {
        let mut raw = RawSettings::default();
        raw.mouse.fine_modifier = "ctrl".into();
        let (cfg, diags) = raw.resolve();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(cfg.mouse.fine_modifier, FineModifier::Ctrl);
    }

    #[test]
    fn duplicate_direct_chord_warns_keeps_first_in_action_order() {
        let mut raw = RawSettings::default();
        raw.shortcuts.bindings = BTreeMap::from([
            ("quit".into(), "Cmd+J".into()),
            ("copy".into(), "Cmd+J".into()),
        ]);
        let (cfg, diags) = raw.resolve();
        assert!(
            diags
                .iter()
                .any(|d| d.message.to_lowercase().contains("duplicate"))
        );
        // NOTE: `copy` precedes `quit` in the fixed action order
        // (`bindings_iter`), so `copy` wins the collision and `quit` is
        // unbound — without this, the outcome would look arbitrary.
        let copy = cfg.shortcuts.copy.as_ref().unwrap().chord();
        assert!(copy.modifiers.meta && copy.key == Key::Char('j'));
        assert!(cfg.shortcuts.quit.is_none());
    }

    #[test]
    fn leader_shadowing_direct_binding_unbinds_shadowed_action() {
        let mut raw = RawSettings::default();
        raw.shortcuts.leader = Some("Cmd+Q".into());
        raw.shortcuts.bindings = BTreeMap::from([("detach-session".into(), "<Leader>d".into())]);
        let (cfg, diags) = raw.resolve();
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("quit") && d.message.contains("shadow"))
        );
        assert!(cfg.shortcuts.quit.is_none());
        assert!(cfg.shortcuts.detach_session.is_some());
    }

    #[test]
    fn empty_leader_disables_leader_without_warning() {
        let mut raw = RawSettings::default();
        raw.shortcuts.leader = Some("".into());
        let (cfg, diags) = raw.resolve();
        assert_eq!(cfg.shortcuts.leader, None);
        assert!(
            !diags.iter().any(|d| d.message.contains("leader")),
            "empty leader must not produce a warning: {diags:?}"
        );
    }

    #[test]
    fn valid_leader_resolves_normally() {
        let mut raw = RawSettings::default();
        raw.shortcuts.leader = Some("Ctrl+A".into());
        let (cfg, diags) = raw.resolve();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            cfg.shortcuts.leader,
            Some(Leader::Chord(parse_key_chord("Ctrl+A").unwrap()))
        );
    }

    #[test]
    fn unmappable_leader_warns_and_reverts_to_default_leader() {
        let mut raw = RawSettings::default();
        raw.shortcuts.leader = Some("Cmd+Plus".into());
        raw.shortcuts.bindings = BTreeMap::from([("detach-session".into(), "<Leader>d".into())]);
        let (cfg, diags) = raw.resolve();
        assert!(diags.iter().any(|d| d.message.contains("physical")));
        assert_eq!(cfg.shortcuts.leader, Shortcuts::default().leader);
    }

    #[test]
    fn vi_mode_duplicate_key_warns_and_keeps_first_action() {
        let mut raw = RawSettings::default();
        raw.vi_mode.bindings = BTreeMap::from([
            ("yank".into(), vec!["x".into()]),
            ("exit".into(), vec!["x".into(), "q".into()]),
        ]);
        let (cfg, diags) = raw.resolve();
        assert!(
            diags
                .iter()
                .any(|d| d.message.to_lowercase().contains("duplicate"))
        );
        // NOTE: `yank` precedes `exit` in the fixed action order, so `yank`
        // keeps `x` and `exit` loses it (but keeps its other key, `q`) —
        // without this, the outcome would look arbitrary.
        assert!(cfg.vi_mode.yank.iter().any(|k| k.to_string() == "x"));
        assert!(!cfg.vi_mode.exit.iter().any(|k| k.to_string() == "x"));
        assert!(cfg.vi_mode.exit.iter().any(|k| k.to_string() == "q"));
    }

    #[test]
    fn default_raw_resolves_to_default_config() {
        let (cfg, diags) = RawSettings::default().resolve();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(cfg, OrzmaConfigs::default());
    }
}
