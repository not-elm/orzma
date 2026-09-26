//! Vi mode: the cursor it adds and the switch that enters or leaves it.

use crate::screen::grid::coords::GridPoint;

/// Vi-mode cursor position in active-grid coordinates.
///
/// The line goes negative while the vi cursor sits in scrollback
/// history. The sign is not a visibility signal: a negative line can
/// still be visible, so project `point` with
/// [`crate::prelude::GridLine::to_viewport`] to decide whether there
/// is a cell to paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViCursor {
    /// Grid cell the vi cursor sits on.
    pub point: GridPoint,
}

/// The direction of a vi-mode switch.
///
/// This terminal does not implement vi mode, so nothing in it acts on a
/// switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViModeSwitch {
    /// Enter vi mode.
    Enter,
    /// Leave vi mode.
    Exit,
}

/// A vi-cursor motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViMotion {
    /// One line up.
    Up,
    /// One line down.
    Down,
    /// One cell left.
    Left,
    /// One cell right.
    Right,
    /// First column of the line.
    First,
    /// Last column of the line.
    Last,
    /// First non-blank column of the line.
    FirstOccupied,
    /// Top line of the viewport.
    High,
    /// Middle line of the viewport.
    Middle,
    /// Bottom line of the viewport.
    Low,
    /// Start of the previous semantic word.
    SemanticLeft,
    /// Start of the next semantic word.
    SemanticRight,
    /// End of the previous semantic word.
    SemanticLeftEnd,
    /// End of the next semantic word.
    SemanticRightEnd,
    /// Start of the previous whitespace-delimited word.
    WordLeft,
    /// Start of the next whitespace-delimited word.
    WordRight,
    /// End of the previous whitespace-delimited word.
    WordLeftEnd,
    /// End of the next whitespace-delimited word.
    WordRightEnd,
    /// Matching bracket of the one under the cursor.
    Bracket,
    /// Previous paragraph break.
    ParagraphUp,
    /// Next paragraph break.
    ParagraphDown,
}
