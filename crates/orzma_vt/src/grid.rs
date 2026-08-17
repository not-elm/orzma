use crate::schema::GridCell;

pub struct Grid {
    rows: Vec<Row>,
}

#[derive(Debug, PartialEq)]
pub struct Row(Vec<GridCell>);
