use ozmux_configs::copy_mode::{CopyModeBaseKey, CopyModeConfig, CopyModeKey};
use ozmux_configs::shortcuts::{Binding, Key, parse_key_chord};
use ozmux_configs::test_support::load_with_overrides;
use ozmux_configs::{OzmuxConfigs, OzmuxConfigsError};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn missing_file_yields_defaults() {
    let nonexistent = fixture("does_not_exist.toml");
    let configs = load_with_overrides(Some(nonexistent), None, None).unwrap();
    let defaults = OzmuxConfigs::default();
    assert_eq!(
        configs.shortcuts.bindings_iter().count(),
        defaults.shortcuts.bindings_iter().count()
    );
}

#[test]
fn empty_file_yields_defaults() {
    let configs = load_with_overrides(Some(fixture("empty.toml")), None, None).unwrap();
    assert_eq!(configs.shortcuts, OzmuxConfigs::default().shortcuts);
    assert!(
        configs.shortcuts.paste.is_some(),
        "paste must have a default binding"
    );
}

#[test]
fn bindings_section_overrides_one_binding_keeps_others() {
    let configs = load_with_overrides(Some(fixture("bindings_replace.toml")), None, None).unwrap();
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
        OzmuxConfigs::default().shortcuts.paste,
        "unspecified bindings must remain at defaults"
    );
}

#[test]
fn duplicate_chord_rejected() {
    let err = load_with_overrides(Some(fixture("duplicate_binding.toml")), None, None).unwrap_err();
    match err {
        OzmuxConfigsError::DuplicateChords(dupes) => {
            assert!(!dupes.is_empty(), "must report at least one duplicate");
        }
        other => panic!("expected DuplicateChords, got {other:?}"),
    }
}

#[test]
fn modifier_binding_accepted() {
    let configs = load_with_overrides(Some(fixture("modifier_binding.toml")), None, None).unwrap();
    let quit = configs
        .shortcuts
        .quit
        .as_ref()
        .expect("modifier_binding fixture rebinds quit")
        .chord();
    assert!(quit.modifiers.shift, "the fixture's binding carries Shift");
}

#[test]
fn syntax_error_surfaces_parse_toml() {
    let path = fixture("syntax_error.toml");
    let err = load_with_overrides(Some(path.clone()), None, None).unwrap_err();
    match err {
        OzmuxConfigsError::ParseToml { path: p, .. } => assert_eq!(p, path),
        other => panic!("expected ParseToml, got {other:?}"),
    }
}

#[test]
fn unknown_action_surfaces_parse_toml() {
    let err = load_with_overrides(Some(fixture("unknown_action.toml")), None, None).unwrap_err();
    assert!(matches!(err, OzmuxConfigsError::ParseToml { .. }));
}

#[test]
fn tmux_action_rebind_and_unbind() {
    let configs =
        load_with_overrides(Some(fixture("tmux_action_binding.toml")), None, None).unwrap();
    assert_eq!(
        configs.shortcuts.split_vertical_pane,
        Some(Binding::Leader(parse_key_chord("g").unwrap()))
    );
    assert_eq!(configs.shortcuts.select_window_5, None);
}

#[test]
fn copy_mode_rebind_and_unbind() {
    let configs = load_with_overrides(Some(fixture("copy_mode_binding.toml")), None, None).unwrap();
    assert_eq!(
        configs.copy_mode.yank,
        vec![CopyModeKey {
            ctrl: false,
            key: CopyModeBaseKey::Char("Y".to_string()),
        }]
    );
    assert!(configs.copy_mode.search_forward.is_empty());
    assert_eq!(
        configs.copy_mode.cursor_left,
        CopyModeConfig::default().cursor_left
    );
}

#[test]
fn duplicate_copy_mode_key_rejected() {
    let err =
        load_with_overrides(Some(fixture("duplicate_copy_mode_key.toml")), None, None).unwrap_err();
    match err {
        OzmuxConfigsError::DuplicateCopyModeKeys(dupes) => {
            assert!(
                dupes
                    .iter()
                    .any(|d| d.actions.contains(&"yank") && d.actions.contains(&"exit"))
            );
        }
        other => panic!("expected DuplicateCopyModeKeys, got {other:?}"),
    }
}

#[test]
fn duplicate_leader_chord_rejected() {
    let err = load_with_overrides(Some(fixture("duplicate_leader_binding.toml")), None, None)
        .unwrap_err();
    match err {
        OzmuxConfigsError::DuplicatePrefixChords(dupes) => {
            assert!(
                dupes
                    .iter()
                    .any(|d| d.actions.contains(&"new-window") && d.actions.contains(&"zoom-pane"))
            );
        }
        other => panic!("expected DuplicatePrefixChords, got {other:?}"),
    }
}
