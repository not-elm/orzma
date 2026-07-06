//! Re-exports of `crate::theme` constants for UI builders. Kept as a thin
//! module so `ui::*` builders read like `palette::ACCENT` rather than
//! reaching across to `crate::theme` directly.

pub(crate) use crate::theme::{
    ACCENT, BACKGROUND, FLAG_WARN, FOREGROUND, MUTED, PANEL, SESSION_BG, VI_MODE_INDICATOR_BG,
    VI_MODE_INDICATOR_FG,
};
