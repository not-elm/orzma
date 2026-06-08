//! Converts the alacritty-free `ozmux_proto` copy-mode vocabulary into the
//! `alacritty_terminal` types the surface driver's `Vt` methods take. Lives in
//! `ozmuxd` (not `ozmux_vt`) because `ozmux_proto` depends on `ozmux_vt`, so the
//! converters cannot live in `ozmux_vt` (dependency cycle).

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::vi_mode::ViMotion;
use ozmux_proto::{CellSide, SelectionKind, ViMotionKind, ViewportPoint};

/// Converts the proto `ViMotionKind` to alacritty's `ViMotion`.
pub(crate) fn vi_motion_kind_to_alacritty(k: ViMotionKind) -> ViMotion {
    match k {
        ViMotionKind::Left => ViMotion::Left,
        ViMotionKind::Right => ViMotion::Right,
        ViMotionKind::Up => ViMotion::Up,
        ViMotionKind::Down => ViMotion::Down,
        ViMotionKind::First => ViMotion::First,
        ViMotionKind::Last => ViMotion::Last,
        ViMotionKind::FirstOccupied => ViMotion::FirstOccupied,
        ViMotionKind::High => ViMotion::High,
        ViMotionKind::Low => ViMotion::Low,
        ViMotionKind::WordRight => ViMotion::WordRight,
        ViMotionKind::WordLeft => ViMotion::WordLeft,
        ViMotionKind::WordRightEnd => ViMotion::WordRightEnd,
    }
}

/// Converts the proto `SelectionKind` to alacritty's `SelectionType`.
pub(crate) fn selection_kind_to_alacritty(k: SelectionKind) -> SelectionType {
    match k {
        SelectionKind::Simple => SelectionType::Simple,
        SelectionKind::Block => SelectionType::Block,
        SelectionKind::Lines => SelectionType::Lines,
        SelectionKind::Semantic => SelectionType::Semantic,
    }
}

/// Converts the proto selection `CellSide` to alacritty's `index::Side`.
pub(crate) fn side_to_alacritty(s: CellSide) -> Side {
    match s {
        CellSide::Left => Side::Left,
        CellSide::Right => Side::Right,
    }
}

/// Converts the proto `ViewportPoint` to alacritty's viewport `Point`.
pub(crate) fn viewport_point_to_alacritty(p: ViewportPoint) -> Point {
    Point::new(Line(p.line), Column(p.col))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proto_copymode_converters_cover_every_variant() {
        use alacritty_terminal::index::Side;
        use alacritty_terminal::selection::SelectionType;
        use alacritty_terminal::vi_mode::ViMotion;
        assert!(matches!(
            selection_kind_to_alacritty(ozmux_proto::SelectionKind::Block),
            SelectionType::Block
        ));
        assert!(matches!(
            selection_kind_to_alacritty(ozmux_proto::SelectionKind::Lines),
            SelectionType::Lines
        ));
        assert!(matches!(
            selection_kind_to_alacritty(ozmux_proto::SelectionKind::Semantic),
            SelectionType::Semantic
        ));
        assert!(matches!(
            selection_kind_to_alacritty(ozmux_proto::SelectionKind::Simple),
            SelectionType::Simple
        ));
        assert!(matches!(
            vi_motion_kind_to_alacritty(ozmux_proto::ViMotionKind::WordRight),
            ViMotion::WordRight
        ));
        assert!(matches!(
            vi_motion_kind_to_alacritty(ozmux_proto::ViMotionKind::FirstOccupied),
            ViMotion::FirstOccupied
        ));
        assert!(matches!(
            side_to_alacritty(ozmux_proto::CellSide::Left),
            Side::Left
        ));
        assert!(matches!(
            side_to_alacritty(ozmux_proto::CellSide::Right),
            Side::Right
        ));
        let p = viewport_point_to_alacritty(ozmux_proto::ViewportPoint { line: 2, col: 4 });
        assert_eq!(p.line.0, 2);
        assert_eq!(p.column.0, 4);
    }
}
