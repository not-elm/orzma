//! Loads `OrzmaConfigs` synchronously at app build time and exposes it as
//! a Bevy Resource. Config validation errors (duplicate direct or prefix
//! chords, duplicate `[vi-mode]` keys, a leader that shadows a direct
//! binding, prefix bindings with no leader, an unmappable leader key, an
//! out-of-range font size, an unparseable `[font]` face `style`) are fatal
//! (exit 2) so a mistake in one field never silently discards the whole
//! config. Parse / IO errors warn and fall back to defaults so the GUI
//! remains startable for users with stale or invalid config files.

use bevy::prelude::*;
use orzma_configs::OrzmaConfigs;

mod groups;

/// Bevy Resource wrapping the resolved `OrzmaConfigs`.
#[derive(Resource, Debug, Default, Deref)]
pub(crate) struct OrzmaConfigsResource(pub(crate) OrzmaConfigs);

/// Bevy Plugin that loads orzma config from disk at `Plugin::build` and
/// inserts it as [`OrzmaConfigsResource`]. Synchronous; uses `std::fs`.
pub(crate) struct OrzmaConfigsPlugin;

impl Plugin for OrzmaConfigsPlugin {
    fn build(&self, app: &mut App) {
        let configs = OrzmaConfigs::load().unwrap_or_else(|err| match &err {
            // File-not-found: empty user config means use defaults. `OrzmaConfigsError::Io` is
            // { path, source }; drill into source.kind() to inspect ErrorKind.
            orzma_configs::OrzmaConfigsError::Io { source, .. }
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                OrzmaConfigs::default()
            }
            // NOTE: config VALIDATION failures must exit(2), not fall through to
            // the warn+default arm below — defaulting would silently discard the
            // user's ENTIRE config (font, mouse, theme, direct bindings), not
            // just the offending field. Only parse / IO errors warn+default.
            orzma_configs::OrzmaConfigsError::DuplicateChords(_)
            | orzma_configs::OrzmaConfigsError::DuplicatePrefixChords(_)
            | orzma_configs::OrzmaConfigsError::DuplicateViModeKeys(_)
            | orzma_configs::OrzmaConfigsError::LeaderShadowsDirectBinding { .. }
            | orzma_configs::OrzmaConfigsError::UnmappableLeader { .. }
            | orzma_configs::OrzmaConfigsError::InvalidFontSize { .. }
            | orzma_configs::OrzmaConfigsError::InvalidFontStyle { .. } => {
                eprintln!("orzma: config is invalid:\n  {err}");
                std::process::exit(2);
            }
            // Any other error (TOML syntax error, stale schema, IO failure): warn + default.
            // This keeps users with stale config files able to start the GUI while signaling
            // the problem in logs.
            _ => {
                tracing::warn!(?err, "configs: load failed, falling back to defaults");
                eprintln!("orzma: shortcut config could not be loaded; using defaults.");
                eprintln!("  {err}");
                let mut source = std::error::Error::source(&err);
                while let Some(cause) = source {
                    eprintln!("  caused by: {cause}");
                    source = cause.source();
                }
                eprintln!(
                    "  Edit ~/.config/orzma/config.toml to fix or remove it to silence this warning."
                );
                OrzmaConfigs::default()
            }
        });
        app.insert_resource(OrzmaConfigsResource(configs));
    }
}

/// Crate-internal mutex guarding `ORZMA_CONFIG` env-var mutations across
/// tests. Any test (in any module) that mutates the process env BEFORE
/// constructing `OrzmaConfigsPlugin` (or anything else that calls
/// `OrzmaConfigs::load`) MUST acquire this guard for the duration
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
        let nonexistent = std::env::temp_dir().join("orzma_configs_no_file_defaults.toml");
        let _ = std::fs::remove_file(&nonexistent);
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::set_var("ORZMA_CONFIG", &nonexistent);
        }

        let mut app = App::new();
        app.add_plugins(OrzmaConfigsPlugin);
        let res = app
            .world()
            .get_resource::<OrzmaConfigsResource>()
            .expect("plugin must insert resource");
        let defaults = OrzmaConfigs::default();
        assert_eq!(res.shortcuts, defaults.shortcuts);

        // SAFETY: env mutation cleanup under the same env_guard.
        unsafe {
            std::env::remove_var("ORZMA_CONFIG");
        }
    }

    #[test]
    fn plugin_falls_back_to_defaults_when_file_not_found() {
        let _guard = env_guard();
        let nonexistent = std::env::temp_dir().join("orzma_configs_does_not_exist.toml");
        let _ = std::fs::remove_file(&nonexistent);
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::set_var("ORZMA_CONFIG", &nonexistent);
        }

        let mut app = App::new();
        app.add_plugins(OrzmaConfigsPlugin);
        let res = app
            .world()
            .get_resource::<OrzmaConfigsResource>()
            .expect("plugin must insert resource on NotFound");
        let defaults = OrzmaConfigs::default();
        assert_eq!(res.shortcuts, defaults.shortcuts);

        // SAFETY: env mutation cleanup under the same env_guard.
        unsafe {
            std::env::remove_var("ORZMA_CONFIG");
        }
    }

    #[test]
    fn plugin_falls_back_to_defaults_on_broken_toml() {
        let _guard = env_guard();
        let tmp = std::env::temp_dir().join("orzma_configs_broken.toml");
        std::fs::write(&tmp, "this = is not valid }{ toml").unwrap();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::set_var("ORZMA_CONFIG", &tmp);
        }

        let mut app = App::new();
        app.add_plugins(OrzmaConfigsPlugin);
        let res = app
            .world()
            .get_resource::<OrzmaConfigsResource>()
            .expect("plugin must still insert a resource on broken-toml fallback");
        let defaults = OrzmaConfigs::default();
        assert_eq!(res.shortcuts, defaults.shortcuts);

        unsafe {
            std::env::remove_var("ORZMA_CONFIG");
        }
        let _ = std::fs::remove_file(&tmp);
    }

    /// Confirms `OrzmaConfigsError::ParseToml` chains down to the underlying
    /// `toml::de::Error`, so the `eprintln!` "caused by:" walker in
    /// `OrzmaConfigsPlugin::build` will surface the field-name detail to
    /// users with a stale config file. Without that walker, the outer Display
    /// only says "failed to parse TOML at ...".
    #[test]
    fn parse_error_chain_surfaces_inner_toml_cause() {
        let _guard = env_guard();
        let tmp = std::env::temp_dir().join("orzma_configs_unknown_field.toml");
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
            std::env::set_var("ORZMA_CONFIG", &tmp);
        }

        let err = OrzmaConfigs::load().expect_err("must error on unknown field");
        // The outer error must wrap an inner cause via Error::source().
        let source = std::error::Error::source(&err);
        // ParseToml carries a toml::de::Error as #[source]; assert that path
        // is taken.
        assert!(
            source.is_some(),
            "ParseToml error must have an inner source (toml::de::Error) so the eprintln! chain walker has something to print"
        );

        unsafe {
            std::env::remove_var("ORZMA_CONFIG");
        }
        let _ = std::fs::remove_file(&tmp);
    }
}
