//! Throwaway spike (deleted in Task 11): confirms bevy::settings round-trips
//! `HashMap` / nested-struct fields, that groups load in `Plugin::build`, and
//! how a real `#[derive(Reflect)] enum` field serializes to TOML. Every claim
//! below is established by an assertion in
//! [`spike_groups_load_at_build_and_round_trip`], unless noted otherwise.
//!
//! ## Findings
//!
//! 1. **Minimal `register_type` set**: `app.register_type::<SpikeGroup>()`
//!    ALONE is sufficient. `#[derive(Reflect)]` generates a
//!    `register_type_dependencies` fn that recursively registers every field
//!    type transitively (nested struct fields, `HashMap<K, V>`, `Vec<T>`,
//!    `Option<T>`, and primitives) via `TypeRegistry::register::<T>()` -- see
//!    `bevy_reflect-0.19.0/src/type_registry.rs:201-206` and
//!    `bevy_reflect_derive-0.19.0/src/registration.rs:18-25`. No separate
//!    `register_type::<HashMap<String, String>>()` /
//!    `register_type::<Vec<String>>()` / `register_type::<Option<String>>()`
//!    calls are required (Task 4 can copy just one `register_type::<Group>()`
//!    call per settings group). Separately, `ReflectSettingsGroup` (from
//!    `bevy::settings`) MUST be `use`d into scope even though it is never
//!    named directly in this file: the `#[reflect(Resource, SettingsGroup,
//!    Default)]` attribute macro expands to an unqualified reference to the
//!    identifier `ReflectSettingsGroup`, which only resolves if it is
//!    imported (confirmed against the real upstream example,
//!    `bevy-0.19.0/examples/app/settings.rs`, which imports it explicitly
//!    even though its body never names it either).
//!    CAVEAT (see [`spike_reflect_auto_register_finds_spike_group_without_explicit_register_type`]):
//!    this workspace's `bevy` dependency keeps default features on, and
//!    bevy's `2d`/`3d` defaults transitively enable `reflect_auto_register`,
//!    which auto-discovers every `#[derive(Reflect)]` type in the linked
//!    binary before any explicit `register_type` call runs. So in this repo
//!    specifically, omitting `register_type::<SpikeGroup>()` entirely still
//!    passes today -- Task 4 should still call it explicitly per group for
//!    robustness/clarity, but a missing call would NOT currently be caught by
//!    a failing test.
//! 2. **Load timing**: `SettingsPlugin::build` loads settings files and
//!    inserts the group resources synchronously, during `Plugin::build`
//!    itself (see `bevy-settings-0.19.0/src/lib.rs:96-125`). The `SpikeGroup`
//!    resource exists immediately after
//!    `app.add_plugins(SettingsPlugin::new(..))` returns -- before any
//!    `App::update()` call.
//! 3. **Round-trip**: `HashMap<String, String>`,
//!    `HashMap<String, Vec<String>>`, and a nested `#[derive(Reflect)]`
//!    struct field (`SpikeFace`) all survive a save-to-disk -> drop the App
//!    -> build a fresh App -> reload cycle with values intact.
//! 4. **Enum spelling**: a real `#[derive(Reflect)] enum` field
//!    (`SpikeToggle`, not itself a `SettingsGroup` -- just a plain field on
//!    one) serializes to TOML as the exact PascalCase Rust variant
//!    identifier, e.g. `enum_like = "Both"` -- not `"both"` or any other
//!    casing/rename. This is corroborated independently by upstream
//!    `bevy-settings-0.19.0`'s own unit test
//!    `test_resources_to_toml_merges_same_group`, which asserts
//!    `refresh_rate == "Fast"` for `CounterRefreshRateSettings::Fast`. This
//!    confirms the store-as-`String` design decision (spec S6.1): a real enum
//!    field round-trips fine value-wise, but its on-disk TOML representation
//!    is capitalized Rust-identifier text, not conventional lowercase
//!    TOML/config style.
//! 5. **Settings filename**: for `SettingsPlugin::new("orzma-spike-test")`
//!    with no `#[settings_group(file = "...")]` override, the file
//!    bevy_settings writes on macOS is:
//!    `~/Library/Preferences/orzma-spike-test/settings.toml`
//!    i.e. `<prefs_dir>/<app_name>/<file>.toml`, where `<file>` defaults to
//!    the literal `"settings"` (see `bevy-settings-0.19.0/src/lib.rs:407` and
//!    `store_fs.rs:52`).

use std::any::TypeId;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use bevy::prelude::*;
use bevy::settings::{ReflectSettingsGroup, SaveSettingsSync, SettingsGroup, SettingsPlugin};

const APP_NAME: &str = "orzma-spike-test";

#[derive(Reflect, Clone, Debug, PartialEq, Default)]
struct SpikeFace {
    family: Option<String>,
    style: Option<String>,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq, Default)]
enum SpikeToggle {
    #[default]
    Neither,
    Left,
    Right,
    Both,
}

#[derive(Resource, SettingsGroup, Reflect, Debug, Default)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "spike")]
struct SpikeGroup {
    scalar: u32,
    enum_like: SpikeToggle,
    face: SpikeFace,
    map: HashMap<String, String>,
    list_map: HashMap<String, Vec<String>>,
}

/// Resolves `~/Library/Preferences/<APP_NAME>/settings.toml`, replicating the
/// macOS branch of `bevy_platform::dirs::preferences_dir` (that function is
/// crate-private to `bevy_platform` and not reachable from `bevy::settings`,
/// so this spike recomputes it to assert the on-disk path directly).
fn spike_settings_file() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set to resolve the macOS prefs dir");
    PathBuf::from(home)
        .join("Library/Preferences")
        .join(APP_NAME)
        .join("settings.toml")
}

/// Deletes the spike's settings file (and, if now empty, its directory) so
/// repeated test runs don't accumulate real files under the developer's
/// `~/Library/Preferences/`.
struct CleanupGuard;

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        let file = spike_settings_file();
        let _ = fs::remove_file(&file);
        if let Some(dir) = file.parent() {
            let _ = fs::remove_dir(dir);
        }
    }
}

#[test]
fn spike_groups_load_at_build_and_round_trip() {
    let _cleanup = CleanupGuard;

    let mut app = App::new();
    app.register_type::<SpikeGroup>();
    app.add_plugins(SettingsPlugin::new(APP_NAME));
    assert!(
        app.world().get_resource::<SpikeGroup>().is_some(),
        "SettingsPlugin must insert the group during Plugin::build"
    );

    {
        let mut group = app.world_mut().resource_mut::<SpikeGroup>();
        group.scalar = 42;
        group.enum_like = SpikeToggle::Both;
        group.face = SpikeFace {
            family: Some("Iosevka".to_string()),
            style: Some("Bold".to_string()),
        };
        group.map.insert("theme".to_string(), "dark".to_string());
        group.list_map.insert(
            "shortcuts".to_string(),
            vec!["cmd+k".to_string(), "cmd+p".to_string()],
        );
    }
    app.world_mut().commands().queue(SaveSettingsSync::Always);
    app.world_mut().flush();

    let settings_file = spike_settings_file();
    assert!(
        settings_file.is_file(),
        "expected settings file at {settings_file:?}"
    );
    let raw = fs::read_to_string(&settings_file).expect("read spike settings.toml");
    println!("--- resolved settings file: {settings_file:?} ---");
    println!("{raw}");

    assert!(
        raw.contains("enum_like = \"Both\""),
        "expected PascalCase enum serialization (`\"Both\"`), got:\n{raw}"
    );
    assert!(
        !raw.contains("\"both\""),
        "enum must not serialize lowercased, got:\n{raw}"
    );

    drop(app);
    let mut reloaded_app = App::new();
    reloaded_app.register_type::<SpikeGroup>();
    reloaded_app.add_plugins(SettingsPlugin::new(APP_NAME));
    let reloaded = reloaded_app
        .world()
        .get_resource::<SpikeGroup>()
        .expect("reloaded SpikeGroup must exist after Plugin::build");

    assert_eq!(reloaded.scalar, 42);
    assert_eq!(reloaded.enum_like, SpikeToggle::Both);
    assert_eq!(
        reloaded.face,
        SpikeFace {
            family: Some("Iosevka".to_string()),
            style: Some("Bold".to_string()),
        }
    );
    assert_eq!(reloaded.map.get("theme").map(String::as_str), Some("dark"));
    assert_eq!(
        reloaded.list_map.get("shortcuts"),
        Some(&vec!["cmd+k".to_string(), "cmd+p".to_string()])
    );
}

/// Documents an important caveat to finding 1: this repo's actual `bevy`
/// dependency (unlike the brief's bare-bones test snippet) does not disable
/// default features, so bevy's `2d`/`3d` defaults pull in `default_app`,
/// which pulls in `reflect_auto_register`. That feature makes
/// `AppTypeRegistry::new_with_derived_types()` (used by `App::default()`,
/// i.e. `App::new()`) auto-register every `#[derive(Reflect)]` type in the
/// linked binary -- so `SpikeGroup` is present in the registry even before
/// any explicit `register_type` call. This means an accidentally-omitted
/// `register_type::<Group>()` in Task 4's real code would currently NOT be
/// caught by a failing test in this repo. The explicit call should still be
/// made (see finding 1) for robustness against that feature combination ever
/// changing, and for clarity -- but be aware it is not exercised as a
/// necessary step by this passing test alone.
#[test]
fn spike_reflect_auto_register_finds_spike_group_without_explicit_register_type() {
    let app = App::new();
    let registered = app
        .world()
        .resource::<AppTypeRegistry>()
        .read()
        .contains(TypeId::of::<SpikeGroup>());
    assert!(
        registered,
        "expected reflect_auto_register (pulled in transitively via bevy's \
         2d/3d default features) to have already registered SpikeGroup \
         before any explicit register_type::<SpikeGroup>() call"
    );
}
