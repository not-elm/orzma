#[cfg(feature = "alacritty")]
use alacritty_terminal::grid::Scroll as AlacrittyScroll;

pub enum Scroll {
    Delta(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

impl Scroll {
    #[cfg(feature = "alacritty")]
    pub fn to_alacritty_scroll(&self) -> AlacrittyScroll {
        match self {
            Self::Delta(n) => AlacrittyScroll::Delta(*n),
            Self::PageUp => AlacrittyScroll::PageUp,
            Self::PageDown => AlacrittyScroll::PageDown,
            Self::Top => AlacrittyScroll::Top,
            Self::Bottom => AlacrittyScroll::Bottom,
        }
    }
}
