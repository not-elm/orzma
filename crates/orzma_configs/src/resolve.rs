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
    /// the losing binding instead of failing the whole config. A user-set
    /// action (its key present in `self.shortcuts.bindings`) always beats a
    /// colliding built-in default; see `first_user_set_or_first`.
    fn resolve_shortcuts(&self, diags: &mut Vec<Diagnostic>) -> Shortcuts {
        let mut table = Table::new();
        self.collect_shortcut_bindings(&mut table, diags);
        let shortcuts = self.build_shortcuts(diags, &table);
        let user_set: BTreeSet<&str> = self.shortcuts.bindings.keys().map(String::as_str).collect();

        // NOTE: `revert_leader` must be decided BEFORE the leader-shadow
        // check runs: when the leader chord itself is about to revert to
        // the built-in default, it never reaches the resolved `Shortcuts`,
        // so there is no shadow left to repair — computing the shadow
        // victim anyway would force-unbind an action that was never
        // actually unreachable.
        let revert_leader = Self::leader_needs_revert(diags, &shortcuts);
        let mut losers = Self::conflicting_shortcut_actions(diags, &shortcuts, &user_set);
        if !revert_leader {
            losers.extend(Self::leader_shadow_victims(diags, &shortcuts));
        }
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
    /// array is pre-validated key-by-key (mirrors `collect_shortcut_bindings`'s
    /// per-entry granularity), so a bad key skips only itself — the
    /// surviving valid keys of that action are still inserted, instead of
    /// discarding the whole action back to its built-in default. Duplicate
    /// keys WITHIN one action's own array are also deduped here (warn, keep
    /// the first occurrence) — a `["k", "k"]` array must not read as a
    /// cross-action collision below (both entries "belong" to the same
    /// winner, so `dupe.actions.iter().filter(|&a| a != winner)` would find
    /// nothing to remove and silently keep the redundant key). If every key
    /// in an action turns out bad, the action is inserted as an empty array
    /// (the user's binding is honored as an explicit unbind), never
    /// silently reverted to the built-in default. Any key collision ACROSS
    /// actions that the merged result still exposes is then repaired by
    /// dropping the key from every losing action but the winner: the first
    /// user-set action (its key present in `self.vi_mode.bindings`) sharing
    /// the key, or — when the whole group is built-in defaults — the first
    /// action in fixed `VI_MODE_ACTION_KEYS` order (see
    /// `first_user_set_or_first`).
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
            let mut seen: Vec<ViModeKey> = Vec::new();
            let mut valid_keys: Vec<String> = Vec::new();
            for k in keys {
                match parse_vi_mode_key(k) {
                    Ok(parsed) if seen.contains(&parsed) => diags.push(Diagnostic {
                        severity: Severity::Warn,
                        message: format!(
                            "vi-mode `{action}` lists key `{k}` more than once; keeping one"
                        ),
                    }),
                    Ok(parsed) => {
                        seen.push(parsed);
                        valid_keys.push(k.clone());
                    }
                    Err(e) => diags.push(Diagnostic {
                        severity: Severity::Warn,
                        message: format!("vi-mode `{action}` key `{k}`: {e}; skipped"),
                    }),
                }
            }
            let values = valid_keys.into_iter().map(Value::String).collect();
            table.insert(action.clone(), Value::Array(values));
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
        let user_set: BTreeSet<&str> = self.vi_mode.bindings.keys().map(String::as_str).collect();
        let mut remaining: BTreeMap<&'static str, Vec<ViModeKey>> = vi_mode
            .bindings_iter()
            .map(|(label, keys, _)| (label, keys.clone()))
            .collect();
        let mut touched = BTreeSet::new();
        for dupe in &dupes {
            let winner = Self::first_user_set_or_first(&dupe.actions, &user_set);
            for loser in dupe.actions.iter().copied().filter(|&a| a != winner) {
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
    /// style must not reach a downstream `src/font` `.expect()`. Finally,
    /// [`FontConfig::faces_with_ignored_style`] flags any face whose
    /// (valid) `style` has no effective family (own or inherited from
    /// `normal`) — the bundled font is used there and `style` is silently
    /// ignored, so a `Warn` diagnostic is pushed per flagged face.
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
        let font = FontConfig {
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
        };
        for face in font.faces_with_ignored_style() {
            diags.push(Diagnostic {
                severity: Severity::Warn,
                message: format!(
                    "[font].{face}.style is set but no family is configured for it; using the bundled font and ignoring style"
                ),
            });
        }
        font
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
    /// leader-scoped bindings, pushing a `Warn` diagnostic for each and
    /// returning the losing action labels to unbind. Within each conflict
    /// group, the winner is picked by `first_user_set_or_first`: a
    /// user-set action (present in `user_set`) always beats a colliding
    /// built-in default; among several user-set actions (or a group that
    /// is entirely built-in defaults), the first action in fixed
    /// (`bindings_iter`) order wins.
    fn conflicting_shortcut_actions(
        diags: &mut Vec<Diagnostic>,
        shortcuts: &Shortcuts,
        user_set: &BTreeSet<&str>,
    ) -> Vec<&'static str> {
        let mut losers = Vec::new();
        for conflicts in [
            shortcuts.validate_no_direct_conflicts(),
            shortcuts.validate_no_leader_conflicts(),
        ] {
            let Err(dupes) = conflicts else { continue };
            for dupe in dupes {
                let winner = Self::first_user_set_or_first(&dupe.actions, user_set);
                for loser in dupe.actions.iter().copied().filter(|&a| a != winner) {
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
        losers
    }

    /// Action labels whose direct binding chord equals a `Leader::Chord`
    /// leader's own chord — the leader intercepts that chord before any of
    /// their direct bindings could ever fire. ALL such direct bindings are
    /// returned, not just the first: with `leader = "Cmd+V"` and a user
    /// binding `copy = "Cmd+V"` colliding with the default `paste =
    /// "Cmd+V"`, `conflicting_shortcut_actions` above already resolves
    /// `copy` as the winner over `paste` — but `copy` itself is ALSO
    /// shadowed by the leader chord, and stopping at the first match would
    /// leave it bound yet unreachable behind the leader. Pushes one `Warn`
    /// diagnostic per victim.
    ///
    /// Not called when the leader itself is about to revert to the
    /// built-in default (see `resolve_shortcuts`): a `Leader::Chord` only
    /// ever exists because the user explicitly set `leader` to a chord
    /// (the built-in default leader is a bare `ModifierTap`, never a
    /// `Chord`), so the leader is already the user-set party whenever this
    /// check fires — but a leader chord that reverts never reaches the
    /// resolved `Shortcuts`, so there is nothing left to shadow.
    fn leader_shadow_victims(
        diags: &mut Vec<Diagnostic>,
        shortcuts: &Shortcuts,
    ) -> Vec<&'static str> {
        let Some(Leader::Chord(leader_chord)) = shortcuts.leader.as_ref() else {
            return Vec::new();
        };
        shortcuts
            .direct_chords()
            .filter(|(_, chord, _)| *chord == leader_chord)
            .map(|(action, _, _)| {
                diags.push(Diagnostic {
                    severity: Severity::Warn,
                    message: format!(
                        "leader chord {leader_chord} shadows the direct binding for `{action}`; unbinding `{action}`"
                    ),
                });
                action
            })
            .collect()
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

    /// Picks the winner of a conflict group already in fixed
    /// (`bindings_iter`) order: the first action present in `user_set`, or
    /// — when none of the group is user-set (the built-in defaults are
    /// conflict-free by design, so this is a safety fallback) — the first
    /// action in the group. Shared by `conflicting_shortcut_actions` and
    /// `resolve_vi_mode`'s duplicate-key repair.
    fn first_user_set_or_first(
        actions: &[&'static str],
        user_set: &BTreeSet<&str>,
    ) -> &'static str {
        actions
            .iter()
            .copied()
            .find(|&action| user_set.contains(action))
            .unwrap_or(actions[0])
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
        // NOTE: both `quit` and `copy` are user-set here (both keys are
        // present in `raw.shortcuts.bindings`), so the user-set-beats-default
        // rule does not distinguish them — the tie-break falls through to
        // fixed action order (`bindings_iter`), where `copy` precedes `quit`,
        // so `copy` wins and `quit` is unbound. See
        // `user_binding_beats_colliding_default` below for the mixed
        // user-set-vs-default case this tie-break rule does NOT cover.
        let copy = cfg.shortcuts.copy.as_ref().unwrap().chord();
        assert!(copy.modifiers.meta && copy.key == Key::Char('j'));
        assert!(cfg.shortcuts.quit.is_none());
    }

    #[test]
    fn user_binding_beats_colliding_default() {
        let mut raw = RawSettings::default();
        // `Cmd+V` is the built-in default chord for `paste`; binding `copy`
        // to it explicitly must make `copy` win, not `paste`, even though
        // `paste` precedes `copy` in fixed action order.
        raw.shortcuts.bindings = BTreeMap::from([("copy".into(), "Cmd+V".into())]);
        let (cfg, diags) = raw.resolve();
        let copy = cfg.shortcuts.copy.as_ref().unwrap().chord();
        assert!(copy.modifiers.meta && copy.key == Key::Char('v'));
        assert!(
            cfg.shortcuts.paste.is_none(),
            "the built-in default must yield to the user-set binding"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Warn && d.message.contains("paste")),
            "a warning must name the losing default `paste`: {diags:?}"
        );
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
        // NOTE: both `yank` and `exit` are user-set here (both keys are
        // present in `raw.vi_mode.bindings`), so the user-set-beats-default
        // rule does not distinguish them — the tie-break falls through to
        // fixed action order (`bindings_iter`), where `yank` precedes `exit`,
        // so `yank` keeps `x` and `exit` loses it (but keeps its other key,
        // `q`). See `user_vi_binding_beats_colliding_default` below for the
        // mixed user-set-vs-default case this tie-break rule does NOT cover.
        assert!(cfg.vi_mode.yank.iter().any(|k| k.to_string() == "x"));
        assert!(!cfg.vi_mode.exit.iter().any(|k| k.to_string() == "x"));
        assert!(cfg.vi_mode.exit.iter().any(|k| k.to_string() == "q"));
    }

    #[test]
    fn user_vi_binding_beats_colliding_default() {
        let mut raw = RawSettings::default();
        // Default `cursor-left` includes key `h`; explicitly binding `yank`
        // to `h` must make `yank` keep it, not the built-in default, even
        // though `cursor-left` precedes `yank` in fixed action order.
        raw.vi_mode.bindings = BTreeMap::from([("yank".into(), vec!["h".into()])]);
        let (cfg, diags) = raw.resolve();
        assert!(cfg.vi_mode.yank.iter().any(|k| k.to_string() == "h"));
        assert!(
            !cfg.vi_mode.cursor_left.iter().any(|k| k.to_string() == "h"),
            "the built-in default must yield the key to the user-set binding"
        );
        assert!(
            cfg.vi_mode
                .cursor_left
                .iter()
                .any(|k| k.to_string() == "ArrowLeft"),
            "cursor-left keeps its other default key"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Warn && d.message.contains("cursor-left")),
            "a warning must name the losing default `cursor-left`: {diags:?}"
        );
    }

    #[test]
    fn default_raw_resolves_to_default_config() {
        let (cfg, diags) = RawSettings::default().resolve();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(cfg, OrzmaConfigs::default());
    }

    // NOTE: the brief's draft for this test asserted `quit` resolves to its
    // built-in default (Cmd+Q). That does not match the actual semantics:
    // `quit` is only a "leader-shadow victim" here because it was
    // explicitly rebound to `Cmd+.` (nothing anywhere in the pipeline
    // reverts a chord collision with `Cmd+.` back to `Cmd+Q`) — once the
    // leader itself reverts (unmappable `.` key) and the fix skips
    // force-unbinding the now-inapplicable shadow victim, `quit` simply
    // keeps its user-set chord `Cmd+.`. Asserting `Cmd+.` here (not
    // `Cmd+Q`) reflects the actual fixed behavior described by F3: "do NOT
    // unbind the leader-shadow victim when the leader reverts".
    #[test]
    fn leader_revert_does_not_unbind_shadow_victim() {
        let mut raw = RawSettings::default();
        raw.shortcuts.leader = Some("Cmd+.".into());
        raw.shortcuts.bindings = BTreeMap::from([("quit".into(), "Cmd+.".into())]);
        let (cfg, diags) = raw.resolve();
        assert!(
            diags.iter().any(|d| d.message.contains("physical")),
            "the leader revert must still warn: {diags:?}"
        );
        assert_eq!(
            cfg.shortcuts.leader,
            Shortcuts::default().leader,
            "the leader must revert to the built-in default"
        );
        let quit = cfg
            .shortcuts
            .quit
            .as_ref()
            .expect("quit must not be force-unbound once the leader itself reverted");
        assert_eq!(quit.chord().to_string(), "Cmd+.");
    }

    #[test]
    fn leader_shadow_unbinds_all_direct_bindings_on_leader_chord() {
        let mut raw = RawSettings::default();
        raw.shortcuts.leader = Some("Cmd+V".into());
        raw.shortcuts.bindings = BTreeMap::from([("copy".into(), "Cmd+V".into())]);
        let (cfg, diags) = raw.resolve();
        assert!(
            cfg.shortcuts.copy.is_none(),
            "copy must not be left silently bound-but-dead behind the leader chord"
        );
        assert!(
            cfg.shortcuts.paste.is_none(),
            "the default paste binding is also shadowed by the same leader chord"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("copy") && d.message.contains("shadow")),
            "a warning must name copy as unbound by the leader shadow: {diags:?}"
        );
        assert_eq!(
            cfg.shortcuts.leader,
            Some(Leader::Chord(parse_key_chord("Cmd+V").unwrap()))
        );
    }

    #[test]
    fn vi_mode_bad_key_in_array_keeps_valid_subset() {
        let mut raw = RawSettings::default();
        raw.vi_mode.bindings =
            BTreeMap::from([("cursor-up".into(), vec!["k".into(), "bad-typo".into()])]);
        let (cfg, diags) = raw.resolve();
        assert_eq!(cfg.vi_mode.cursor_up, vec![parse_vi_mode_key("k").unwrap()]);
        assert!(
            diags.iter().any(|d| d.message.contains("bad-typo")),
            "a warning must name the bad key: {diags:?}"
        );
    }

    #[test]
    fn vi_mode_all_bad_keys_unbinds_action_instead_of_reverting_to_default() {
        let mut raw = RawSettings::default();
        raw.vi_mode.bindings = BTreeMap::from([("cursor-up".into(), vec!["bad-typo".into()])]);
        let (cfg, diags) = raw.resolve();
        assert!(
            cfg.vi_mode.cursor_up.is_empty(),
            "an action whose only key(s) are bad must resolve as unbound, not reverted to the \
             built-in default"
        );
        assert!(diags.iter().any(|d| d.message.contains("bad-typo")));
    }

    #[test]
    fn vi_mode_duplicate_key_within_one_action_is_deduped() {
        let mut raw = RawSettings::default();
        raw.vi_mode.bindings = BTreeMap::from([("cursor-up".into(), vec!["k".into(), "k".into()])]);
        let (cfg, diags) = raw.resolve();
        assert_eq!(cfg.vi_mode.cursor_up, vec![parse_vi_mode_key("k").unwrap()]);
        assert!(
            diags.iter().any(|d| d.message.contains("more than once")),
            "a warning must flag the repeated key: {diags:?}"
        );
    }

    #[test]
    fn font_style_without_family_warns() {
        let mut raw = RawSettings::default();
        raw.font.bold.style = Some("Bold".into());
        let (cfg, diags) = raw.resolve();
        assert_eq!(
            cfg.font.bold.style.as_deref(),
            Some("Bold"),
            "a valid style is still applied even though it will be ignored downstream"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Warn && d.message.contains("bold")),
            "a warning must name the face whose style is set without a family: {diags:?}"
        );
    }
}
