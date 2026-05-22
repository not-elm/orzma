//! Loads `OzmuxConfigs` synchronously at app build time and exposes it as
//! a Bevy Resource. Falls back to defaults on any I/O or parse error so
//! the GUI never refuses to start because of a malformed config.

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
        let configs = OzmuxConfigs::load_blocking().unwrap_or_else(|err| {
            tracing::warn!(?err, "configs: load failed, using defaults");
            OzmuxConfigs::default()
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
        assert_eq!(res.shortcuts.prefix, defaults.shortcuts.prefix);
    }

    #[test]
    fn plugin_falls_back_to_defaults_on_broken_toml() {
        let _guard = env_guard();
        let tmp = std::env::temp_dir().join("ozmux_configs_resource_broken.toml");
        std::fs::write(&tmp, "this = is not valid }{ toml").unwrap();
        // SAFETY: env mutations are serialized by ENV_GUARD for this crate's tests.
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &tmp);
        }

        let mut app = App::new();
        app.add_plugins(OzmuxConfigsPlugin);
        let res = app
            .world()
            .get_resource::<OzmuxConfigsResource>()
            .expect("plugin must still insert a resource on load failure");
        let defaults = OzmuxConfigs::default();
        assert_eq!(
            res.shortcuts.prefix, defaults.shortcuts.prefix,
            "fallback must use defaults"
        );

        // SAFETY: cleanup under the same env guard.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
        let _ = std::fs::remove_file(&tmp);
    }
}
