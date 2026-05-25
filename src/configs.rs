//! Loads `OzmuxConfigs` synchronously at app build time and exposes it as
//! a Bevy Resource. Chord-conflict errors (DuplicateChords) are fatal
//! (exit 2). Parse / IO errors warn and fall back to defaults so the GUI
//! remains startable for users with stale or invalid config files.

use bevy::prelude::*;
use ozmux_configs::OzmuxConfigs;

/// Bevy Resource wrapping the resolved `OzmuxConfigs`.
#[derive(Resource, Debug, Default, Deref)]
pub(crate) struct OzmuxConfigsResource(pub(crate) OzmuxConfigs);

/// Bevy Plugin that loads ozmux config from disk at `Plugin::build` and
/// inserts it as [`OzmuxConfigsResource`]. Synchronous; uses `std::fs`,
/// no tokio runtime is created here.
pub(crate) struct OzmuxConfigsPlugin;

impl Plugin for OzmuxConfigsPlugin {
    fn build(&self, app: &mut App) {
        let configs = OzmuxConfigs::load_blocking().unwrap_or_else(|err| match &err {
            // File-not-found: empty user config means use defaults. `OzmuxConfigsError::Io` is
            // { path, source }; drill into source.kind() to inspect ErrorKind.
            ozmux_configs::OzmuxConfigsError::Io { source, .. }
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                OzmuxConfigs::default()
            }
            // Chord conflicts are an architectural failure that must be surfaced loudly.
            ozmux_configs::OzmuxConfigsError::DuplicateChords(_) => {
                eprintln!("ozmux-gui: shortcut config has duplicate chords:\n  {err}");
                std::process::exit(2);
            }
            // Any other error (TOML syntax error, stale schema, IO failure): warn + default.
            // This keeps users with stale config files able to start the GUI while signaling
            // the problem in logs.
            _ => {
                tracing::warn!(?err, "configs: load failed, falling back to defaults");
                eprintln!(
                    "ozmux-gui: shortcut config could not be loaded; using defaults.\n  {err}\n  \
                     Edit ~/.config/ozmux/config.toml to fix or remove it to silence this warning."
                );
                OzmuxConfigs::default()
            }
        });
        app.insert_resource(OzmuxConfigsResource(configs));
    }
}

/// Crate-internal mutex guarding `OZMUX_CONFIG` env-var mutations across
/// tests. Any test (in any module) that mutates the process env BEFORE
/// constructing `OzmuxConfigsPlugin` (or anything else that calls
/// `OzmuxConfigs::load_blocking`) MUST acquire this guard for the duration
/// of the construction.
#[cfg(test)]
pub(crate) fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::Mutex;
    static ENV_GUARD: Mutex<()> = Mutex::new(());
    ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_inserts_configs_resource_matching_defaults_when_no_config_file() {
        let _guard = env_guard();
        // SAFETY: env mutations are serialized by ENV_GUARD for this crate's tests.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }

        let mut app = App::new();
        app.add_plugins(OzmuxConfigsPlugin);
        let res = app
            .world()
            .get_resource::<OzmuxConfigsResource>()
            .expect("plugin must insert resource");
        let defaults = OzmuxConfigs::default();
        assert_eq!(res.shortcuts.bindings, defaults.shortcuts.bindings);
    }

    #[test]
    fn plugin_falls_back_to_defaults_when_file_not_found() {
        let _guard = env_guard();
        let nonexistent = std::env::temp_dir().join("ozmux_configs_does_not_exist.toml");
        let _ = std::fs::remove_file(&nonexistent);
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &nonexistent);
        }

        let mut app = App::new();
        app.add_plugins(OzmuxConfigsPlugin);
        let res = app
            .world()
            .get_resource::<OzmuxConfigsResource>()
            .expect("plugin must insert resource on NotFound");
        let defaults = OzmuxConfigs::default();
        assert_eq!(res.shortcuts.bindings, defaults.shortcuts.bindings);

        // SAFETY: env mutation cleanup under the same env_guard.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
    }

    #[test]
    fn plugin_falls_back_to_defaults_on_broken_toml() {
        let _guard = env_guard();
        let tmp = std::env::temp_dir().join("ozmux_configs_broken.toml");
        std::fs::write(&tmp, "this = is not valid }{ toml").unwrap();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &tmp);
        }

        let mut app = App::new();
        app.add_plugins(OzmuxConfigsPlugin);
        let res = app
            .world()
            .get_resource::<OzmuxConfigsResource>()
            .expect("plugin must still insert a resource on broken-toml fallback");
        let defaults = OzmuxConfigs::default();
        assert_eq!(res.shortcuts.bindings, defaults.shortcuts.bindings);

        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
        let _ = std::fs::remove_file(&tmp);
    }
}
