use crate::screen::cell::Pen;

#[derive(Default)]
pub(super) struct ScreenState {
    pub line: u16,
    pub column: u16,
    pub pending_wrap: bool,
    pub pen: Pen,
}
