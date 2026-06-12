//! Re-exports of `crate::theme` constants for UI builders. Kept as a thin
//! module so `ui::*` builders read like `palette::ACCENT` rather than
//! reaching across to `crate::theme` directly.

pub(crate) use crate::theme::{
    ACCENT, BACKGROUND, BORDER, COPY_MODE_INDICATOR_BG, COPY_MODE_INDICATOR_FG, FOREGROUND, MUTED,
    PANEL, SURFACE_EXTENSION, SURFACE_TERMINAL, TAB_ACTIVE_BG, TAB_BAR_BG,
};
