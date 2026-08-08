#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Position {
    pub x: usize,
    pub y: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionRange {
    pub start: Position,
    pub end: Position,
}
