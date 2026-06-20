use ozmux_configs::shortcuts::Key;
use ozmux_configs::test_support::load_with_overrides;
use ozmux_configs::{OzmuxConfigs, OzmuxConfigsError};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[tokio::test]
async fn missing_file_yields_defaults() {
    let nonexistent = fixture("does_not_exist.toml");
    let configs = load_with_overrides(Some(nonexistent), None, None)
        .await
        .unwrap();
    let defaults = OzmuxConfigs::default();
    assert_eq!(
        configs.shortcuts.bindings.iter().count(),
        defaults.shortcuts.bindings.iter().count()
    );
}

#[tokio::test]
async fn empty_file_yields_defaults() {
    let configs = load_with_overrides(Some(fixture("empty.toml")), None, None)
        .await
        .unwrap();
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

#[tokio::test]
async fn bindings_section_overrides_one_binding_keeps_others() {
    let configs = load_with_overrides(Some(fixture("bindings_replace.toml")), None, None)
        .await
        .unwrap();
    let close = configs
        .shortcuts
        .bindings
        .close_pane
        .as_ref()
        .expect("bindings_replace fixture rebinds close-pane");
    assert_eq!(close.key, Key::Char('y'));
    assert!(close.modifiers.meta, "Cmd modifier must be set");
    let defaults = OzmuxConfigs::default();
    assert_eq!(
        configs.shortcuts.bindings.focus_pane_left, defaults.shortcuts.bindings.focus_pane_left,
        "unspecified bindings must remain at defaults"
    );
}

#[tokio::test]
async fn theme_patch_preserves_other_fields() {
    let configs = load_with_overrides(Some(fixture("theme_accent.toml")), None, None)
        .await
        .unwrap();
    assert_eq!(configs.theme.accent, "#deadbe");
    let defaults = OzmuxConfigs::default();
    assert_eq!(configs.theme.background, defaults.theme.background);
    assert_eq!(configs.theme.foreground, defaults.theme.foreground);
    assert_eq!(configs.theme.border, defaults.theme.border);
    assert_eq!(configs.theme.destructive, defaults.theme.destructive);
}

#[tokio::test]
async fn duplicate_chord_rejected() {
    let err = load_with_overrides(Some(fixture("duplicate_binding.toml")), None, None)
        .await
        .unwrap_err();
    match err {
        OzmuxConfigsError::DuplicateChords(dupes) => {
            assert!(!dupes.is_empty(), "must report at least one duplicate");
        }
        other => panic!("expected DuplicateChords, got {other:?}"),
    }
}

#[tokio::test]
async fn modifier_binding_accepted() {
    let configs = load_with_overrides(Some(fixture("modifier_binding.toml")), None, None)
        .await
        .unwrap();
    let close = configs
        .shortcuts
        .bindings
        .close_pane
        .as_ref()
        .expect("modifier_binding fixture rebinds close-pane");
    assert!(close.modifiers.shift, "the fixture's binding carries Shift");
}

#[tokio::test]
async fn syntax_error_surfaces_parse_toml() {
    let err = load_with_overrides(Some(fixture("syntax_error.toml")), None, None)
        .await
        .unwrap_err();
    match err {
        OzmuxConfigsError::ParseToml { path, .. } => {
            assert!(path.ends_with("syntax_error.toml"));
        }
        other => panic!("expected ParseToml, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_action_surfaces_parse_toml() {
    let err = load_with_overrides(Some(fixture("unknown_action.toml")), None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, OzmuxConfigsError::ParseToml { .. }));
}

#[test]
fn load_blocking_missing_file_yields_defaults() {
    use ozmux_configs::test_support::load_blocking_with_overrides;
    let nonexistent = fixture("does_not_exist.toml");
    let configs = load_blocking_with_overrides(Some(nonexistent), None, None).unwrap();
    let defaults = OzmuxConfigs::default();
    assert_eq!(
        configs.shortcuts.bindings.iter().count(),
        defaults.shortcuts.bindings.iter().count()
    );
}

#[test]
fn load_blocking_syntax_error_returns_parse_toml_with_path() {
    use ozmux_configs::test_support::load_blocking_with_overrides;
    let path = fixture("syntax_error.toml");
    let err = load_blocking_with_overrides(Some(path.clone()), None, None).unwrap_err();
    match err {
        OzmuxConfigsError::ParseToml { path: p, .. } => assert_eq!(p, path),
        other => panic!("expected ParseToml, got {other:?}"),
    }
}
