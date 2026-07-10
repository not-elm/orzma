//! Resolves the `bevy::settings` `SettingsGroup` resources
//! (`configs::groups`) into `OrzmaConfigs` at `Plugin::build` and exposes
//! the result as a Bevy Resource. Resolution diagnostics (duplicate direct
//! or prefix chords, duplicate `[vi-mode]` keys, a leader that shadows a
//! direct binding, prefix bindings with no leader, an unmappable leader
//! key, an out-of-range font size, an unparseable `[font]` face `style`)
//! are logged via `tracing::warn!` and the offending entries fall back to
//! defaults — nothing here is fatal, so the GUI always starts.

use bevy::prelude::*;
use bevy::settings::SettingsPlugin;
use groups::{
    FontSettings, InactivePaneSettings, KeyboardSettings, MouseSettings, OrzmaSettings,
    ScrollbackSettings, ShortcutSettings, ViModeSettings,
};
use orzma_configs::OrzmaConfigs;

mod groups;
mod migrate;

/// Bevy Resource wrapping the resolved `OrzmaConfigs`.
#[derive(Resource, Debug, Default, Deref)]
pub(crate) struct OrzmaConfigsResource(pub(crate) OrzmaConfigs);

/// Bevy Plugin that resolves orzma config from the `bevy::settings` groups
/// at `Plugin::build` and inserts the result as [`OrzmaConfigsResource`].
pub(crate) struct OrzmaConfigsPlugin;

impl Plugin for OrzmaConfigsPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: register_type BEFORE SettingsPlugin (else map/nested fields load empty).
        app.register_type::<ShortcutSettings>()
            .register_type::<ViModeSettings>()
            .register_type::<FontSettings>()
            .register_type::<MouseSettings>()
            .register_type::<KeyboardSettings>()
            .register_type::<InactivePaneSettings>()
            .register_type::<ScrollbackSettings>()
            .register_type::<OrzmaSettings>()
            .add_plugins(SettingsPlugin::new("orzma"));
        migrate::migrate_if_needed(app.world_mut());
        resolve_and_insert(app.world_mut());
    }
}

/// Crate-internal mutex guarding `ORZMA_CONFIG` env-var mutations across
/// tests. Any test (in any module) that mutates the process env BEFORE
/// constructing `OrzmaConfigsPlugin` (or anything else that resolves the
/// legacy config path, e.g. `orzma_configs::path::resolve_config_path`)
/// MUST acquire this guard for the duration of the construction.
#[cfg(test)]
pub(crate) fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::Mutex;
    static ENV_GUARD: Mutex<()> = Mutex::new(());
    ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner())
}

/// Reads the settings groups (or `$ORZMA_CONFIG`, Task 8), resolves them,
/// logs diagnostics, and inserts [`OrzmaConfigsResource`]. Extracted from
/// `build` so tests can exercise it without adding `SettingsPlugin` (which
/// reads the real OS prefs dir).
fn resolve_and_insert(world: &mut World) {
    let raw = groups::collect_raw(world);
    let (cfg, diags) = raw.resolve();
    for d in &diags {
        tracing::warn!(target: "orzma::config", "{}", d.message);
    }
    world.insert_resource(OrzmaConfigsResource(cfg));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::ffi::OsString;
    use tempfile::TempDir;

    #[test]
    fn resolve_and_insert_produces_default_resource() {
        // NOTE: Hermetic test: no SettingsPlugin, no disk. collect_raw falls back to Default
        // for any group not present, so an empty world resolves to the defaults.
        let mut app = App::new();
        resolve_and_insert(app.world_mut());
        let res = app
            .world()
            .get_resource::<OrzmaConfigsResource>()
            .expect("resource inserted");
        assert_eq!(res.0, OrzmaConfigs::default());
    }

    /// RAII guard for a process-environment variable. `EnvVarGuard::set` /
    /// `::unset` snapshot the prior value and restore it (or remove the var,
    /// if it was previously unset) on drop, even on panic. Duplicated from
    /// `src/font.rs`'s test-only `EnvVarGuard` rather than shared: that one
    /// is private to `font.rs`'s own test module.
    ///
    /// The caller MUST hold [`env_guard`] for the full lifetime of every
    /// `EnvVarGuard` to keep env mutations serialized across tests.
    struct EnvVarGuard {
        key: &'static str,
        prior: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let prior = std::env::var_os(key);
            // SAFETY: caller holds env_guard() for the duration of this guard.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prior }
        }

        fn unset(key: &'static str) -> Self {
            let prior = std::env::var_os(key);
            // SAFETY: caller holds env_guard() for the duration of this guard.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, prior }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: caller still holds env_guard() (LIFO drop order keeps
            // this ahead of the MutexGuard's own drop within each test scope).
            unsafe {
                match self.prior.take() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    /// Retained regression test for the riskiest mechanism in the
    /// `bevy::settings` migration: `HashMap`/`Vec` binding maps round-tripping
    /// through a REAL `SettingsPlugin` disk load. This is deliberately NOT a
    /// hermetic bypass (unlike `resolve_and_insert_produces_default_resource`
    /// above, which skips `SettingsPlugin` entirely) and NOT the migration
    /// path (skipped here via a pre-written `.migrated` marker, so this
    /// exercises the steady-state load instead).
    ///
    /// Writes a `settings.toml` in the exact on-disk shape `bevy::settings`
    /// itself would have written, at the exact path
    /// `bevy_platform::dirs::preferences_dir()` resolves to under an
    /// isolated `$HOME`, then runs the real `OrzmaConfigsPlugin` (the same
    /// `register_type` -> `SettingsPlugin` -> `migrate_if_needed` ->
    /// `resolve_and_insert` sequence `main.rs` runs) and asserts the
    /// `HashMap<String, String>` (`[shortcuts.bindings]`) and
    /// `HashMap<String, Vec<String>>` (`[vi-mode.bindings]`) fields both
    /// reached the resolved `OrzmaConfigsResource`.
    #[test]
    fn steady_state_settings_toml_round_trips_maps_through_real_settings_plugin() {
        let _guard = env_guard();
        let tempdir = TempDir::new().expect("create isolated $HOME");
        let _home = EnvVarGuard::set("HOME", tempdir.path());
        let _xdg_config_home = EnvVarGuard::set("XDG_CONFIG_HOME", tempdir.path().join(".config"));
        let _orzma_config = EnvVarGuard::unset("ORZMA_CONFIG");

        let prefs_dir = bevy_platform::dirs::preferences_dir()
            .expect("preferences dir resolves under the isolated $HOME");
        let settings_dir = prefs_dir.join("orzma");
        std::fs::create_dir_all(&settings_dir).expect("create settings dir");
        std::fs::write(
            settings_dir.join("settings.toml"),
            "[shortcuts.bindings]\n\
             quit = \"Cmd+Shift+Q\"\n\
             \n\
             [vi-mode.bindings]\n\
             cursor-down = [\"k\", \"ArrowUp\"]\n",
        )
        .expect("write settings.toml");
        std::fs::write(settings_dir.join(".migrated"), "").expect("write migration marker");

        let mut app = App::new();
        app.add_plugins(OrzmaConfigsPlugin);

        let res = app
            .world()
            .get_resource::<OrzmaConfigsResource>()
            .expect("OrzmaConfigsPlugin must insert OrzmaConfigsResource");

        let quit = res
            .shortcuts
            .quit
            .as_ref()
            .expect("quit must still be bound")
            .chord();
        assert_eq!(
            quit.to_string(),
            "Cmd+Shift+Q",
            "the HashMap<String, String> [shortcuts.bindings] entry must survive a real disk load"
        );
        assert_ne!(
            res.shortcuts.quit,
            OrzmaConfigs::default().shortcuts.quit,
            "must be the disk override (Cmd+Shift+Q), not the built-in default (Cmd+Q)"
        );

        let cursor_down: Vec<String> = res
            .vi_mode
            .cursor_down
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            cursor_down,
            vec!["k".to_string(), "ArrowUp".to_string()],
            "the HashMap<String, Vec<String>> [vi-mode.bindings] entry must survive a real \
             disk load, with both keys present in the Vec"
        );
    }
}
