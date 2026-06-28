//! Per-mode processing. Each application mode declares its systems inside a
//! dedicated submodule; `AppMode` (added here in a later step) selects which.

pub(crate) mod tmux;
