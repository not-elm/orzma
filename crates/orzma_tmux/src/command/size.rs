//! Client/window size-declaration commands sent to tmux on resize.

use tmux_control::TmuxCommand;
use tmux_control_parser::WindowId;

/// `refresh-client -C <cols>,<rows>` — declares this client's global cell size.
pub struct RefreshClient {
    /// Terminal width in columns.
    pub cols: u16,
    /// Terminal height in rows.
    pub rows: u16,
}
impl TmuxCommand for RefreshClient {
    fn into_raw_command(self) -> String {
        format!("refresh-client -C {},{}", self.cols, self.rows)
    }
}

/// `refresh-client -C @<win>:<cols>x<rows>` — per-window size (tmux ≥ 3.4).
pub struct WindowRefreshClient {
    /// Target window id.
    pub win: WindowId,
    /// Window width in columns.
    pub cols: u16,
    /// Window height in rows.
    pub rows: u16,
}
impl TmuxCommand for WindowRefreshClient {
    fn into_raw_command(self) -> String {
        format!(
            "refresh-client -C @{}:{}x{}",
            self.win.0, self.cols, self.rows
        )
    }
}

/// `resize-window -x <cols> -y <rows> -t @<win>` — the tmux < 3.4 per-window fallback.
///
/// # Invariants
///
/// tmux sets the session's `window-size` option to `manual` as a side effect.
pub struct ResizeWindow {
    /// Target window id.
    pub win: WindowId,
    /// Window width in columns.
    pub cols: u16,
    /// Window height in rows.
    pub rows: u16,
}
impl TmuxCommand for ResizeWindow {
    fn into_raw_command(self) -> String {
        format!(
            "resize-window -x {} -y {} -t @{}",
            self.cols, self.rows, self.win.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_client_uses_comma_size_form() {
        assert_eq!(
            RefreshClient { cols: 80, rows: 24 }.into_raw_command(),
            "refresh-client -C 80,24"
        );
    }

    #[test]
    fn window_refresh_client_uses_per_window_form() {
        assert_eq!(
            WindowRefreshClient {
                win: WindowId(2),
                cols: 80,
                rows: 24
            }
            .into_raw_command(),
            "refresh-client -C @2:80x24"
        );
    }

    #[test]
    fn resize_window_targets_window() {
        assert_eq!(
            ResizeWindow {
                win: WindowId(2),
                cols: 80,
                rows: 24
            }
            .into_raw_command(),
            "resize-window -x 80 -y 24 -t @2"
        );
    }
}
