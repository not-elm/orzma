//! Bridge between `OzmuxConfigsResource.font` and the renderer's
//! `TerminalFonts` Resource, plus the `TerminalUiFont` handle that UI
//! text builders consume. Runs at Startup, ordered
//! `.before(TerminalFontInitSet::InitCellMetrics)` so the renderer's
//! cell-metrics computation sees any overridden font.
//!
//! Startup-only: font changes require a process restart. If a future
//! feature adds config hot-reload, `bridge_font_config` must move to a
//! change-detection system in Update (and additionally re-issue cell
//! metrics + invalidate the glyph atlas — see the renderer crate).

use crate::configs::OzmuxConfigsResource;
use bevy::prelude::*;
use bevy::text::Font;
use bevy_terminal_renderer::{FontFace, TerminalFontInitSet, TerminalFonts};
use ozmux_configs::font::FontConfig;
use ozmux_configs::path::{SystemEnv, expand_user_path};
use std::path::Path;

/// Strong handle to the UI font asset (regular face). Inserted by
/// `bridge_font_config`. UI builders read this resource to set
/// `TextFont { font, ... }`.
#[derive(Resource, Clone)]
pub(crate) struct TerminalUiFont(pub Handle<Font>);

/// Bevy plugin that wires `bridge_font_config` into Startup.
pub(crate) struct FontBridgePlugin;

impl Plugin for FontBridgePlugin {
    fn build(&self, app: &mut App) {
        // NOTE: must run before TerminalFontInitSet::InitCellMetrics so
        // the metrics computed inside the renderer's
        // init_cell_metrics_from_primary_window reflect any user-supplied
        // font override.
        app.add_systems(
            Startup,
            bridge_font_config.before(TerminalFontInitSet::InitCellMetrics),
        );
    }
}

const BUNDLED_REGULAR: &[u8] = include_bytes!(
    "../assets/fonts/iosevka/IosevkaTermNerdFontMono-Regular.ttf"
);
const BUNDLED_BOLD: &[u8] = include_bytes!(
    "../assets/fonts/iosevka/IosevkaTermNerdFontMono-Bold.ttf"
);
const BUNDLED_ITALIC: &[u8] = include_bytes!(
    "../assets/fonts/iosevka/IosevkaTermNerdFontMono-Italic.ttf"
);
const BUNDLED_BOLD_ITALIC: &[u8] = include_bytes!(
    "../assets/fonts/iosevka/IosevkaTermNerdFontMono-BoldItalic.ttf"
);

/// Loads bytes for one face. Returns the file contents if `path` is set
/// AND expansion succeeds AND read succeeds; otherwise returns the
/// bundled bytes. Failures are warned, not propagated — partial override
/// is a feature.
fn load_face_bytes(path: Option<&Path>, bundled: &'static [u8], face: FontFace) -> Vec<u8> {
    let Some(p) = path else {
        return bundled.to_vec();
    };
    let Some(expanded) = expand_user_path(p, &SystemEnv) else {
        tracing::warn!(?face, path = %p.display(), "font path expansion failed; using bundled");
        return bundled.to_vec();
    };
    match std::fs::read(&expanded) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(?face, path = %expanded.display(), %err, "font path read failed; using bundled");
            bundled.to_vec()
        }
    }
}

/// Validates one face's bytes by attempting to parse them as a font.
/// Returns the original bytes on success; on failure (parse error), warns
/// and returns the bundled bytes for that face.
fn validate_or_bundled(bytes: Vec<u8>, bundled: &'static [u8], face: FontFace) -> Vec<u8> {
    match ab_glyph::FontArc::try_from_vec(bytes.clone()) {
        Ok(_) => bytes,
        Err(err) => {
            tracing::warn!(
                ?face,
                %err,
                "font face failed to parse; substituting bundled bytes for THAT face only"
            );
            bundled.to_vec()
        }
    }
}

fn bridge_font_config(
    mut commands: Commands,
    configs: Res<OzmuxConfigsResource>,
    mut fonts_assets: ResMut<Assets<Font>>,
    mut terminal_fonts: ResMut<TerminalFonts>,
) {
    let font: &FontConfig = &configs.font;

    let regular_bytes = load_face_bytes(
        font.normal_path.as_deref(),
        BUNDLED_REGULAR,
        FontFace::Regular,
    );
    let bold_bytes =
        load_face_bytes(font.bold_path.as_deref(), BUNDLED_BOLD, FontFace::Bold);
    let italic_bytes = load_face_bytes(
        font.italic_path.as_deref(),
        BUNDLED_ITALIC,
        FontFace::Italic,
    );
    let bold_italic_bytes = load_face_bytes(
        font.bold_italic_path.as_deref(),
        BUNDLED_BOLD_ITALIC,
        FontFace::BoldItalic,
    );

    // NOTE: validate-per-face is load-bearing. Without it, a corrupt
    // override on ANY face drops ALL overrides — the all-or-nothing
    // regression that commit 09213e7 fixed.
    let regular_bytes = validate_or_bundled(regular_bytes, BUNDLED_REGULAR, FontFace::Regular);
    let bold_bytes = validate_or_bundled(bold_bytes, BUNDLED_BOLD, FontFace::Bold);
    let italic_bytes = validate_or_bundled(italic_bytes, BUNDLED_ITALIC, FontFace::Italic);
    let bold_italic_bytes = validate_or_bundled(
        bold_italic_bytes,
        BUNDLED_BOLD_ITALIC,
        FontFace::BoldItalic,
    );

    let ui_regular_bytes = regular_bytes.clone();

    let new_fonts = TerminalFonts::from_bytes(
        regular_bytes,
        bold_bytes,
        italic_bytes,
        bold_italic_bytes,
    )
    .expect("validated bytes must parse");
    *terminal_fonts = new_fonts;

    let handle = match Font::try_from_bytes(ui_regular_bytes) {
        Ok(font_asset) => fonts_assets.add(font_asset),
        Err(err) => {
            tracing::warn!(%err, "bevy_text::Font::try_from_bytes rejected the regular face; \
                              falling back to Bevy's default font (FiraMono via default_font)");
            Handle::<Font>::default()
        }
    };
    commands.insert_resource(TerminalUiFont(handle));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::Font as AbFont;
    use crate::configs::OzmuxConfigsPlugin;
    use bevy::asset::AssetPlugin;
    use bevy::window::{PrimaryWindow, Window, WindowResolution};
    use bevy_terminal_renderer::TerminalFontPlugin;
    use std::io::Write;

    fn make_test_app() -> (App, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::configs::env_guard();
        // SAFETY: env mutations serialized by env_guard().
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<Font>();
        // Spawn a PrimaryWindow so init_cell_metrics_from_primary_window
        // (which is registered as a Startup system by TerminalFontPlugin)
        // can run without panicking. We don't actually exercise the
        // metrics in these tests — just need the system not to skip.
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        (app, guard)
    }

    #[test]
    fn default_config_keeps_bundled_iosevka_in_terminal_fonts() {
        let (mut app, _guard) = make_test_app();
        app.update();
        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data(),
            BUNDLED_REGULAR,
            "regular face is bundled Iosevka when no override is configured"
        );
    }

    #[test]
    fn default_config_inserts_terminal_ui_font_with_strong_handle() {
        let (mut app, _guard) = make_test_app();
        app.update();
        let ui_font = app
            .world()
            .get_resource::<TerminalUiFont>()
            .expect("TerminalUiFont must be inserted on the cold path");
        // A strong handle from Assets::add stores the asset under the
        // returned handle. Look it up to verify it resolves.
        let assets = app.world().resource::<Assets<Font>>();
        assert!(
            assets.get(&ui_font.0).is_some(),
            "TerminalUiFont handle must resolve to an asset stored in Assets<Font>"
        );
    }

    #[test]
    fn configured_normal_path_overrides_regular_face() {
        // Use the bundled Iosevka regular bytes as the "override" — the
        // test verifies the bytes flow through std::fs::read, not that
        // they differ from bundled. Write to a temp file and point
        // OZMUX_CONFIG at a TOML that uses it.
        let tmp_dir = std::env::temp_dir().join("ozmux_font_bridge_test_normal");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let ttf_path = tmp_dir.join("regular.ttf");
        std::fs::write(&ttf_path, BUNDLED_REGULAR).expect("write temp ttf");

        let toml_path = tmp_dir.join("config.toml");
        let mut f = std::fs::File::create(&toml_path).expect("create toml");
        writeln!(
            f,
            "[font.normal]\npath = \"{}\"\n",
            ttf_path.to_string_lossy()
        )
        .expect("write toml");
        drop(f);

        let _guard = crate::configs::env_guard();
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &toml_path);
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data(),
            BUNDLED_REGULAR,
            "regular face bytes equal the override file (which happens to contain bundled bytes)"
        );

        let _ = std::fs::remove_file(&ttf_path);
        let _ = std::fs::remove_file(&toml_path);
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
    }

    #[test]
    fn missing_normal_path_falls_back_to_bundled() {
        let nonexistent = std::env::temp_dir()
            .join("ozmux_font_bridge_test_missing.ttf");
        let _ = std::fs::remove_file(&nonexistent);
        let toml = std::env::temp_dir()
            .join("ozmux_font_bridge_test_missing.toml");
        std::fs::write(
            &toml,
            format!("[font.normal]\npath = \"{}\"\n", nonexistent.to_string_lossy()),
        )
        .expect("write toml");

        let _guard = crate::configs::env_guard();
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &toml);
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data(),
            BUNDLED_REGULAR,
            "regular face falls back to bundled when configured path does not exist"
        );

        let _ = std::fs::remove_file(&toml);
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
    }

    #[test]
    fn normal_path_set_does_not_inherit_to_bold() {
        let tmp_dir = std::env::temp_dir().join("ozmux_font_bridge_test_no_inherit");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let normal_path = tmp_dir.join("only-regular.ttf");
        std::fs::write(&normal_path, BUNDLED_REGULAR).expect("write");
        let toml = tmp_dir.join("config.toml");
        std::fs::write(
            &toml,
            format!(
                "[font.normal]\npath = \"{}\"\n",
                normal_path.to_string_lossy()
            ),
        )
        .expect("write toml");

        let _guard = crate::configs::env_guard();
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &toml);
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.bold.font_data(),
            BUNDLED_BOLD,
            "bold face must NOT inherit from normal_path; it stays bundled bold"
        );

        let _ = std::fs::remove_file(&normal_path);
        let _ = std::fs::remove_file(&toml);
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
    }

    #[test]
    fn corrupt_bold_path_falls_back_per_face_without_dropping_normal_override() {
        // Write a corrupt "bold" TTF (just random bytes that won't parse)
        // and a valid "normal" TTF (bundled Iosevka regular). Verify
        // bridge_font_config keeps the normal override AND replaces only
        // the corrupt bold with bundled bold.
        let tmp_dir = std::env::temp_dir().join("ozmux_font_bridge_test_corrupt_bold");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let normal_path = tmp_dir.join("regular.ttf");
        std::fs::write(&normal_path, BUNDLED_REGULAR).expect("write regular");
        let bold_path = tmp_dir.join("bold.ttf");
        std::fs::write(&bold_path, b"this is not a valid TTF").expect("write corrupt bold");
        let toml = tmp_dir.join("config.toml");
        std::fs::write(
            &toml,
            format!(
                "[font.normal]\npath = \"{}\"\n[font.bold]\npath = \"{}\"\n",
                normal_path.to_string_lossy(),
                bold_path.to_string_lossy(),
            ),
        )
        .expect("write toml");

        let _guard = crate::configs::env_guard();
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::set_var("OZMUX_CONFIG", &toml);
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        // Regular must remain the user-supplied (= bundled-regular bytes
        // here, but reached via the override path).
        assert_eq!(
            fonts.regular.font_data(),
            BUNDLED_REGULAR,
            "normal override must survive a corrupt bold override"
        );
        // Bold must fall back to bundled because the user-supplied bold
        // is corrupt.
        assert_eq!(
            fonts.bold.font_data(),
            BUNDLED_BOLD,
            "corrupt bold override must fall back to bundled bold"
        );

        let _ = std::fs::remove_file(&normal_path);
        let _ = std::fs::remove_file(&bold_path);
        let _ = std::fs::remove_file(&toml);
        // SAFETY: serialized by env_guard().
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
    }
}
