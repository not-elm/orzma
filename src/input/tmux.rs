//! tmux-mode input dispatch: keyboard forwarding, mouse gestures, per-pane
//! input gates, IME/mouse forwarding to tmux, pane hit-testing, and window-bar
//! input. The complementary tmux state and rendering live in `crate::mode::tmux`.

pub(crate) mod forward;
pub(crate) mod gate;
pub(crate) mod input;
pub(crate) mod mouse;
mod pane_hit;
pub(crate) mod window_bar_input;
