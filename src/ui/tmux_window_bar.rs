//! The tmux window status bar: a bottom row showing the session name and the
//! window list (`<index>:<name>`), with the active window highlighted and each
//! entry clickable to `select-window`.

/// Formats one window list entry, e.g. `0:zsh`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the window bar rebuild in phase-3b T5")
)]
fn window_label(index: u32, name: &str) -> String {
    format!("{index}:{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_label_formats_index_and_name() {
        assert_eq!(window_label(0, "zsh"), "0:zsh");
        assert_eq!(window_label(12, ""), "12:");
    }
}
