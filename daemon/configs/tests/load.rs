use ozmux_configs::shortcuts::{Action, Key};
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
        configs.shortcuts.bindings.len(),
        defaults.shortcuts.bindings.len()
    );
}

#[tokio::test]
async fn empty_file_yields_defaults() {
    let configs = load_with_overrides(Some(fixture("empty.toml")), None, None)
        .await
        .unwrap();
    let defaults = OzmuxConfigs::default();
    assert_eq!(
        configs.shortcuts.bindings.len(),
        defaults.shortcuts.bindings.len()
    );
    assert!(matches!(
        configs.shortcuts.bindings[0].action,
        Action::ClosePane
    ));
}

#[tokio::test]
async fn prefix_override_keeps_default_bindings() {
    let configs = load_with_overrides(Some(fixture("prefix_only.toml")), None, None)
        .await
        .unwrap();
    assert_eq!(configs.shortcuts.prefix.chord.key, Key::Char('a'));
    assert_eq!(configs.shortcuts.prefix.timeout_ms, 3000);
    let defaults = OzmuxConfigs::default();
    assert_eq!(
        configs.shortcuts.bindings.len(),
        defaults.shortcuts.bindings.len()
    );
    assert!(matches!(
        configs.shortcuts.bindings[0].action,
        Action::ClosePane
    ));
}

#[tokio::test]
async fn bindings_section_fully_replaces_defaults() {
    let configs = load_with_overrides(Some(fixture("bindings_replace.toml")), None, None)
        .await
        .unwrap();
    assert_eq!(configs.shortcuts.bindings.len(), 1);
    assert_eq!(configs.shortcuts.bindings[0].chord.key, Key::Char('y'));
    assert!(matches!(
        configs.shortcuts.bindings[0].action,
        Action::CloseWindow
    ));
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
async fn duplicate_binding_rejected() {
    let err = load_with_overrides(Some(fixture("duplicate_binding.toml")), None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, OzmuxConfigsError::DuplicateBinding { .. }));
}

#[tokio::test]
async fn modifier_binding_accepted() {
    let configs = load_with_overrides(Some(fixture("modifier_binding.toml")), None, None)
        .await
        .unwrap();
    assert!(
        configs
            .shortcuts
            .bindings
            .iter()
            .any(|b| b.chord.modifiers.shift),
        "a shift-modifier binding must load successfully"
    );
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
