//! Loads `OrzmaConfigs` synchronously at app build time and exposes it as
//! a Bevy Resource. Validation errors exit the process (code 2); parse and
//! IO errors warn and fall back to defaults.

use bevy::prelude::*;
use bevy_orzma_tty_renderer::prelude::CaretStyle;
use orzma_configs::OrzmaConfigs;
use orzma_configs::cursor::{CursorConfig, CursorStyleSetting};
use orzma_configs::mouse::MouseConfig;
use orzma_tty::prelude::WheelConfig;
use orzma_vt::prelude::{CursorBlink, CursorPolicy, CursorShape, TextCursorStyle};

/// The resolved `OrzmaConfigs`, loaded once at app build time.
#[derive(Resource, Debug, Default, Deref)]
pub(crate) struct OrzmaConfigsResource(pub(crate) OrzmaConfigs);

/// Loads orzma config from disk and inserts it as [`OrzmaConfigsResource`];
/// synchronous, using `std::fs`.
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
        let caret = caret_style(&configs.cursor);
        app.insert_resource(OrzmaConfigsResource(configs))
            .insert_resource(caret);
    }
}

/// The wheel-routing policy the backend applies, from the `[mouse]`
/// block.
pub(crate) fn wheel_config(mc: &MouseConfig) -> WheelConfig {
    WheelConfig {
        lines_per_notch: mc.lines_per_notch,
        fine_lines: mc.fine_lines,
        max_protocol_events_per_frame: mc.max_protocol_events_per_frame,
    }
}

/// The renderer's caret drawing knobs, from the `[cursor]` section.
pub(crate) fn caret_style(config: &CursorConfig) -> CaretStyle {
    CaretStyle {
        blink_interval: config.blink_interval(),
        blink_timeout: config.blink_timeout(),
        thickness: config.thickness(),
        unfocused_hollow: config.unfocused_hollow,
    }
}

/// The VT-layer cursor policy the `[cursor]` section selects.
pub(crate) fn cursor_policy(config: &CursorConfig) -> CursorPolicy {
    CursorPolicy {
        initial: TextCursorStyle {
            shape: match config.style {
                CursorStyleSetting::Block => CursorShape::Block,
                CursorStyleSetting::Underline => CursorShape::Underline,
                CursorStyleSetting::Bar => CursorShape::Bar,
            },
            blink: CursorBlink::Blinking,
        },
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

    /// Asserts that each `[mouse]` wheel field lands on its `WheelConfig`
    /// counterpart.
    ///
    /// Case: a user sets `lines_per_notch = 5`, `fine_lines = 2`, and
    /// `max_protocol_events_per_frame = 16` in config.toml.
    #[test]
    fn wheel_config_maps_the_mouse_block() {
        let mc = MouseConfig {
            lines_per_notch: 5,
            fine_lines: 2,
            max_protocol_events_per_frame: 16,
            ..MouseConfig::default()
        };
        let out = wheel_config(&mc);
        assert_eq!(out.lines_per_notch, 5);
        assert_eq!(out.fine_lines, 2);
        assert_eq!(out.max_protocol_events_per_frame, 16);
    }

    /// Asserts that `cursor_policy` maps each `[cursor]` style setting
    /// to its `CursorShape` counterpart, and that the initial caret
    /// blinks.
    ///
    /// Case: a user selects each `style` in the `[cursor]` section of
    /// their config.toml in turn.
    #[test]
    fn cursor_policy_maps_every_style_setting() {
        let cases = [
            (CursorStyleSetting::Block, CursorShape::Block),
            (CursorStyleSetting::Underline, CursorShape::Underline),
            (CursorStyleSetting::Bar, CursorShape::Bar),
        ];
        for (style, expected_shape) in cases {
            let mut config = CursorConfig::default();
            config.style = style;
            let policy = cursor_policy(&config);
            assert_eq!(policy.initial.shape, expected_shape);
            assert_eq!(policy.initial.blink, CursorBlink::Blinking);
        }
    }

    /// Asserts that `caret_style` carries each `[cursor]` drawing knob
    /// into its own `CaretStyle` field, resolving the timings through
    /// the config's accessors.
    ///
    /// Case: a user turns the hollow unfocused caret off and leaves the
    /// shipped blink timings alone.
    #[test]
    fn caret_style_maps_every_drawing_knob() {
        let mut config = CursorConfig::default();
        config.unfocused_hollow = false;
        let style = caret_style(&config);
        assert_eq!(style.blink_interval, config.blink_interval());
        assert_eq!(style.blink_timeout, config.blink_timeout());
        assert_eq!(style.thickness, config.thickness());
        assert!(!style.unfocused_hollow);
    }
}
