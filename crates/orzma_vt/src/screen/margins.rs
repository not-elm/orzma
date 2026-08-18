/// DECSTBM scroll region; `bottom` is the inclusive last row index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Margins {
    /// First row of the scroll region (0 = top of screen).
    pub top: u16,
    /// Inclusive last row of the scroll region (default `rows - 1`).
    pub bottom: u16,
}

impl Margins {
    pub fn new(rows: u16) -> Self {
        Self {
            top: 0,
            bottom: rows - 1,
        }
    }
}
