//! Mode lifecycle: the Default-mode single-PTY shell (`default`) and the tmux
//! control-mode connection lifecycle (`tmux`). Owns mode transitions.

pub(crate) mod default;
pub(crate) mod tmux;
