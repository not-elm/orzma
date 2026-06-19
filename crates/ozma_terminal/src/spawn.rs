//! Terminal component types and dimension helpers for Ozma mode.

use bevy::prelude::*;

/// Marker component identifying the single Ozma-mode terminal entity.
///
/// Exactly one entity carries this marker while the terminal is active.
#[derive(Component)]
pub struct OzmaTerminal;

/// Shell override resource.
///
/// `None` means fall back to `$SHELL` at spawn time.
#[derive(Resource)]
pub struct OzmaTerminalConfig {
    /// Optional shell path. When set, overrides `$SHELL` and `/bin/sh`.
    pub shell: Option<String>,
}

/// Computes terminal dimensions in cells from physical pixel size.
///
/// Returns `(cols, rows)`, each clamped to a minimum of 1.
pub fn cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let cols = ((w_px as f32 / cell_w).floor() as u16).max(1);
    let rows = ((h_px as f32 / cell_h).floor() as u16).max(1);
    (cols, rows)
}

/// Resolves the shell path: config → `$SHELL` → `/bin/sh`.
pub fn resolve_shell(config: Option<&str>, env_shell: Option<&str>) -> String {
    config
        .filter(|s| !s.is_empty())
        .or_else(|| env_shell.filter(|s| !s.is_empty()))
        .unwrap_or("/bin/sh")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_resolution_uses_config() {
        assert_eq!(
            resolve_shell(Some("/bin/fish"), Some("/bin/zsh")),
            "/bin/fish"
        );
    }

    #[test]
    fn shell_resolution_falls_back_to_env() {
        assert_eq!(resolve_shell(None, Some("/bin/zsh")), "/bin/zsh");
    }

    #[test]
    fn shell_resolution_falls_back_to_sh() {
        assert_eq!(resolve_shell(None, None), "/bin/sh");
    }

    #[test]
    fn cells_for_divides_and_floors() {
        assert_eq!(cells_for(800, 600, 8.0, 16.0), (100, 37));
        assert_eq!(cells_for(0, 0, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(807, 607, 8.0, 16.0), (100, 37));
    }
}
