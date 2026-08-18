//! One row of elements, ordered left to right.

use std::ops::{Deref, DerefMut, Index, IndexMut};

/// A single row of `T`, left to right.
///
/// Storage rows are `Row<Cell>` and emitted rows are [`Row<Run>`], so
/// the two differ only in what one element spans: a cell is one
/// column, a run is as many as its text.
///
/// The element type is deliberately not defaulted — a bare `Row` in a
/// wire struct silently meaning `Row<Cell>` is exactly the mistake
/// that would compile and then fail far from its cause.
///
/// [`Row<Run>`]: crate::screen::grid::run::Run
#[derive(Debug, Clone, PartialEq)]
pub struct Row<T>(Vec<T>);

impl<T: Clone> Row<T> {
    /// Builds a row of `len` copies of `fill`.
    pub fn filled(len: u16, fill: T) -> Self {
        Self(vec![fill; usize::from(len)])
    }
}

impl<T> From<Vec<T>> for Row<T> {
    fn from(elements: Vec<T>) -> Self {
        Self(elements)
    }
}

impl<T> Deref for Row<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Row<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Indexes the element at a 0-based position.
///
/// The position is a column only for `Row<Cell>`; one
/// [`Run`](crate::screen::grid::run::Run) spans as many columns as its
/// text is wide.
impl<T> Index<u16> for Row<T> {
    type Output = T;

    fn index(&self, position: u16) -> &T {
        &self.0[usize::from(position)]
    }
}

impl<T> IndexMut<u16> for Row<T> {
    fn index_mut(&mut self, position: u16) -> &mut T {
        &mut self.0[usize::from(position)]
    }
}
