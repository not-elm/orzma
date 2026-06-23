use ozmux_configs::shortcuts::Key;
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
        configs.shortcuts.bindings.iter().count(),
        defaults.shortcuts.bindings.iter().count()
    );
}

#[test]
fn empty_file_yields_defaults() {
    let configs = load_with_overrides(Some(fixture("empty.toml")), None, None).unwrap();
    let defaults = OzmuxConfigs::default();
    assert_eq!(
        configs.shortcuts.bindings.iter().count(),
        defaults.shortcuts.bindings.iter().count()
    );
    assert!(
        configs.shortcuts.bindings.paste.is_some(),
        "paste must have a default binding"
    );
}

#[test]
fn bindings_section_overrides_one_binding_keeps_others() {
    let configs = load_with_overrides(Some(fixture("bindings_replace.toml")), None, None).unwrap();
    let quit = configs
        .shortcuts
        .bindings
        .quit
        .as_ref()
        .expect("bindings_replace fixture rebinds quit");
    assert_eq!(quit.key, Key::Char('y'));
    assert!(quit.modifiers.meta, "Cmd modifier must be set");
    let defaults = OzmuxConfigs::default();
    assert_eq!(
        configs.shortcuts.bindings.paste, defaults.shortcuts.bindings.paste,
        "unspecified bindings must remain at defaults"
    );
}

#[test]
fn theme_patch_preserves_other_fields() {
    let configs = load_with_overrides(Some(fixture("theme_accent.toml")), None, None).unwrap();
    assert_eq!(configs.theme.accent, "#deadbe");
    let defaults = OzmuxConfigs::default();
    assert_eq!(configs.theme.background, defaults.theme.background);
    assert_eq!(configs.theme.foreground, defaults.theme.foreground);
    assert_eq!(configs.theme.border, defaults.theme.border);
    assert_eq!(configs.theme.destructive, defaults.theme.destructive);
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
        .bindings
        .quit
        .as_ref()
        .expect("modifier_binding fixture rebinds quit");
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
