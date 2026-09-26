//! Loads the config files under `tests/fixtures` through the same resolve,
//! parse, normalize, and validate path as `OrzmaConfigs::load`.

use crate::path::{self, Env};
use crate::shortcuts::{Binding, Key, parse_key_chord};
use crate::vi_mode::{ViModeBaseKey, ViModeConfig, ViModeKey};
use crate::{OrzmaConfigs, OrzmaConfigsError, OrzmaConfigsResult};
use std::path::PathBuf;

/// An environment whose only variable is `$ORZMA_CONFIG`, pointing at one file.
struct FixtureEnv(PathBuf);

impl Env for FixtureEnv {
    fn var(&self, key: &str) -> Option<String> {
        (key == path::ENV_ORZMA_CONFIG).then(|| self.0.to_string_lossy().into_owned())
    }

    fn home_dir(&self) -> Option<PathBuf> {
        None
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load_fixture(name: &str) -> OrzmaConfigsResult<OrzmaConfigs> {
    OrzmaConfigs::load_with_env(&FixtureEnv(fixture(name)))
}

/// Asserts that a config path with no file behind it loads the defaults.
///
/// Case: a user who never created `config.toml` starts orzma.
#[test]
fn missing_file_yields_defaults() {
    let configs = load_fixture("does_not_exist.toml").expect("a missing file is not an error");
    assert_eq!(
        configs.shortcuts.bindings_iter().count(),
        OrzmaConfigs::default().shortcuts.bindings_iter().count()
    );
}

/// Asserts that an empty file loads the default shortcuts.
///
/// Case: a user creates an empty `config.toml` to fill in later.
#[test]
fn empty_file_yields_defaults() {
    let configs = load_fixture("empty.toml").expect("an empty file is valid");
    assert_eq!(configs.shortcuts, OrzmaConfigs::default().shortcuts);
    assert!(
        configs.shortcuts.paste.is_some(),
        "paste must have a default binding"
    );
}

/// Asserts that rebinding one action keeps every other action at its default.
///
/// Case: a user moves `quit` to `Cmd+Y` and changes nothing else.
#[test]
fn bindings_section_overrides_one_binding_keeps_others() {
    let configs = load_fixture("bindings_replace.toml").expect("the fixture is valid");
    let quit = configs
        .shortcuts
        .quit
        .as_ref()
        .expect("bindings_replace fixture rebinds quit")
        .chord();
    assert_eq!(quit.key, Key::Char('y'));
    assert!(quit.modifiers.meta, "Cmd modifier must be set");
    assert_eq!(
        configs.shortcuts.paste,
        OrzmaConfigs::default().shortcuts.paste,
        "unspecified bindings must remain at defaults"
    );
}

/// Asserts that two actions sharing a direct chord fail with `DuplicateChords`.
///
/// Case: a user binds `release-webview-focus` to the paste chord by mistake.
#[test]
fn duplicate_chord_rejected() {
    let err = load_fixture("duplicate_binding.toml").expect_err("the fixture has a duplicate");
    match err {
        OrzmaConfigsError::DuplicateChords(dupes) => {
            assert!(!dupes.is_empty(), "must report at least one duplicate");
        }
        other => panic!("expected DuplicateChords, got {other:?}"),
    }
}

/// Asserts that a chord with an extra modifier is accepted as written.
///
/// Case: a user binds `quit` to `Cmd+Shift+W`.
#[test]
fn modifier_binding_accepted() {
    let configs = load_fixture("modifier_binding.toml").expect("the fixture is valid");
    let quit = configs
        .shortcuts
        .quit
        .as_ref()
        .expect("modifier_binding fixture rebinds quit")
        .chord();
    assert!(quit.modifiers.shift, "the fixture's binding carries Shift");
}

/// Asserts that a TOML syntax error surfaces as `ParseToml` naming the file.
///
/// Case: a user leaves a table header unclosed in `config.toml`.
#[test]
fn syntax_error_surfaces_parse_toml() {
    let path = fixture("syntax_error.toml");
    let err = OrzmaConfigs::load_with_env(&FixtureEnv(path.clone()))
        .expect_err("the fixture is malformed");
    match err {
        OrzmaConfigsError::ParseToml { path: p, .. } => assert_eq!(p, path),
        other => panic!("expected ParseToml, got {other:?}"),
    }
}

/// Asserts that an action name orzma does not know surfaces as `ParseToml`.
///
/// Case: a user misspells an action name under `[shortcuts]`.
#[test]
fn unknown_action_surfaces_parse_toml() {
    let err = load_fixture("unknown_action.toml").expect_err("the fixture names no action");
    assert!(matches!(err, OrzmaConfigsError::ParseToml { .. }));
}

/// Asserts that a pane action can be rebound to a leader chord and another
/// action can be unbound with an empty string.
///
/// Case: a user moves the vertical split to `<Leader>g` and turns off one
/// action they never use.
#[test]
fn multiplexer_action_rebind_and_unbind() {
    let configs = load_fixture("multiplexer_action_binding.toml").expect("the fixture is valid");
    assert_eq!(
        configs.shortcuts.split_vertical_pane,
        Some(Binding::Leader {
            chord: parse_key_chord("g").expect("`g` is a valid chord"),
            repeat: false,
        })
    );
    assert_eq!(configs.shortcuts.select_window_5, None);
}

/// Asserts that a vi-mode key can be rebound and another unbound, leaving the
/// rest at their defaults.
///
/// Case: a user moves yank to `Y` and turns off forward search.
#[test]
fn vi_mode_rebind_and_unbind() {
    let configs = load_fixture("vi_mode_binding.toml").expect("the fixture is valid");
    assert_eq!(
        configs.vi_mode.yank,
        vec![ViModeKey {
            ctrl: false,
            key: ViModeBaseKey::Char("Y".to_string()),
        }]
    );
    assert!(configs.vi_mode.search_forward.is_empty());
    assert_eq!(
        configs.vi_mode.cursor_left,
        ViModeConfig::default().cursor_left
    );
}

/// Asserts that two vi-mode actions sharing a key fail with
/// `DuplicateViModeKeys` naming both.
///
/// Case: a user gives yank and exit the same key.
#[test]
fn duplicate_vi_mode_key_rejected() {
    let err = load_fixture("duplicate_vi_mode_key.toml").expect_err("the fixture has a duplicate");
    match err {
        OrzmaConfigsError::DuplicateViModeKeys(dupes) => {
            assert!(
                dupes
                    .iter()
                    .any(|d| d.actions.contains(&"yank") && d.actions.contains(&"exit"))
            );
        }
        other => panic!("expected DuplicateViModeKeys, got {other:?}"),
    }
}

/// Asserts that two actions sharing a leader chord fail with
/// `DuplicatePrefixChords` naming both.
///
/// Case: a user binds two actions to `<Leader>g`.
#[test]
fn duplicate_leader_chord_rejected() {
    let err =
        load_fixture("duplicate_leader_binding.toml").expect_err("the fixture has a duplicate");
    match err {
        OrzmaConfigsError::DuplicatePrefixChords(dupes) => {
            assert!(
                dupes
                    .iter()
                    .any(|d| d.actions.contains(&"new-window") && d.actions.contains(&"zoom-pane"))
            );
        }
        other => panic!("expected DuplicatePrefixChords, got {other:?}"),
    }
}
