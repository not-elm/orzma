//! Loads `OzmuxConfigs` synchronously at app build time and exposes it as
//! a Bevy Resource. Shortcut-config validation errors (duplicate direct or
//! prefix chords, a leader that shadows a direct binding, prefix bindings with
//! no leader) are fatal (exit 2). Parse / IO errors warn and fall back to
//! defaults so the GUI remains startable for users with stale or invalid
//! config files.

use bevy::prelude::*;
use ozmux_configs::OzmuxConfigs;

/// Bevy Resource wrapping the resolved `OzmuxConfigs`.
#[derive(Resource, Debug, Default, Deref)]
pub(crate) struct OzmuxConfigsResource(pub(crate) OzmuxConfigs);

/// Bevy Plugin that loads ozmux config from disk at `Plugin::build` and
/// inserts it as [`OzmuxConfigsResource`]. Synchronous; uses `std::fs`.
pub(crate) struct OzmuxConfigsPlugin;

impl Plugin for OzmuxConfigsPlugin {
    fn build(&self, app: &mut App) {
        let configs = OzmuxConfigs::load().unwrap_or_else(|err| match &err {
            // File-not-found: empty user config means use defaults. `OzmuxConfigsError::Io` is
            // { path, source }; drill into source.kind() to inspect ErrorKind.
            ozmux_configs::OzmuxConfigsError::Io { source, .. }
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                OzmuxConfigs::default()
            }
            // NOTE: shortcut-config validation failures must exit(2), not fall
            // through to the warn+default arm below — defaulting would silently
            // discard the user's ENTIRE config (font, mouse, theme, direct
            // bindings), not just the offending shortcut.
            ozmux_configs::OzmuxConfigsError::DuplicateChords(_)
            | ozmux_configs::OzmuxConfigsError::DuplicatePrefixChords(_)
            | ozmux_configs::OzmuxConfigsError::PrefixBindingsWithoutLeader
            | ozmux_configs::OzmuxConfigsError::LeaderShadowsDirectBinding { .. } => {
                eprintln!("ozmux: shortcut config is invalid:\n  {err}");
                std::process::exit(2);
            }
            // Any other error (TOML syntax error, stale schema, IO failure): warn + default.
            // This keeps users with stale config files able to start the GUI while signaling
            // the problem in logs.
            _ => {
                tracing::warn!(?err, "configs: load failed, falling back to defaults");
                eprintln!("ozmux: shortcut config could not be loaded; using defaults.");
                eprintln!("  {err}");
                let mut source = std::error::Error::source(&err);
                while let Some(cause) = source {
                    eprintln!("  caused by: {cause}");
                    source = cause.source();
                }
                eprintln!(
                    "  Edit ~/.config/ozmux/config.toml to fix or remove it to silence this warning."
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
/// `OzmuxConfigs::load`) MUST acquire this guard for the duration
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

    /// Confirms `OzmuxConfigsError::ParseToml` chains down to the underlying
    /// `toml::de::Error`, so the `eprintln!` "caused by:" walker in
    /// `OzmuxConfigsPlugin::build` will surface the field-name detail to
    /// users with a stale config file. Without that walker, the outer Display
    /// only says "failed to parse TOML at ...".
    #[test]
    fn parse_error_chain_surfaces_inner_toml_cause() {
        let _guard = env_guard();
        let tmp = std::env::temp_dir().join("ozmux_configs_unknown_field.toml");
        // Provide a key that won't match any expected struct field. The
        // exact wording of the toml parser's error message is not pinned;
        // we only assert that the chain has a non-empty source.
        std::fs::write(
            &tmp,
            "this-is-not-a-valid-section = 42\n[unknown-section]\nfoo = 1\n",
        )
        .unwrap();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &tmp);
        }

        let err = OzmuxConfigs::load().expect_err("must error on unknown field");
        // The outer error must wrap an inner cause via Error::source().
        let source = std::error::Error::source(&err);
        // ParseToml carries a toml::de::Error as #[source]; assert that path
        // is taken.
        assert!(
            source.is_some(),
            "ParseToml error must have an inner source (toml::de::Error) so the eprintln! chain walker has something to print"
        );

        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
        let _ = std::fs::remove_file(&tmp);
    }
}
