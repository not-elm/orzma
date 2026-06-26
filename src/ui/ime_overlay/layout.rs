//! Pure pixel-math for the IME preedit overlay: grapheme cell layout, caret /
//! clause cell offsets, and the window-anchored overlay position. No Bevy ECS
//! — unit-testable without an `App`.

use bevy::math::Vec2;
use ozma_tty_renderer::CellMetrics;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Computes the overlay's top-left logical-pixel position relative to the
/// window origin. Caller writes this into `Node.left` / `Node.top`.
///
/// All metric inputs are physical px; the function does the physical→logical
/// conversion via `scale`. The overlay sits at the cursor row (Alacritty
/// parity), clamped so its right edge stays inside the host rect, then so its
/// left edge does not escape the host's left side.
pub(super) fn compute_overlay_pos(
    ui_global_translation_phys: Vec2,
    host_size_phys: Vec2,
    cursor_cell: (u16, u16),
    metrics: &CellMetrics,
    measured_width_logical: f32,
    scale: f32,
) -> Vec2 {
    // NOTE: `UiGlobalTransform.translation` is the CENTER of the node in
    // PHYSICAL pixels; subtract `0.5 * host_size_phys` for the top-left. Do NOT
    // multiply by `scale` — translation is already physical.
    let cell_w_phys = metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.line_height_phys.floor().max(1.0);
    let host_top_left_phys = ui_global_translation_phys - 0.5 * host_size_phys;
    let cell_origin_phys = host_top_left_phys
        + Vec2::new(
            cursor_cell.0 as f32 * cell_w_phys,
            cursor_cell.1 as f32 * cell_h_phys,
        );
    let pos_logical = cell_origin_phys / scale;

    let host_top_left_logical = host_top_left_phys / scale;
    let host_size_logical = host_size_phys / scale;
    let host_left = host_top_left_logical.x;
    let host_right = host_left + host_size_logical.x;
    let mut left = pos_logical.x;
    if left + measured_width_logical > host_right {
        left = host_right - measured_width_logical;
    }
    if left < host_left {
        left = host_left;
    }

    Vec2::new(left, pos_logical.y)
}

/// Returns `(begin_cells, end_cells)` — the per-side cell offsets of the IME
/// caret/clause range relative to the start of `text`. Fullwidth CJK counts as
/// 2 cells per glyph, matching the renderer's width logic.
pub(super) fn caret_cell_offsets(text: &str, (begin, end): (usize, usize)) -> (f32, f32) {
    (
        clamped_prefix_cells(&text[..begin]) as f32,
        clamped_prefix_cells(&text[..end]) as f32,
    )
}

/// Total clamped cell width of `text` — the sum of [`clamp_cluster_cells`] over
/// its grapheme clusters, mirroring [`layout_preedit_cells`] so caret/clause
/// offsets cannot diverge from the rendered cells for a wide cluster.
fn clamped_prefix_cells(text: &str) -> u32 {
    text.graphemes(true).map(clamp_cluster_cells).sum()
}

/// A single placed preedit cell-unit: the grapheme cluster's text and its left
/// edge in logical px (the cell origin it is anchored to).
pub(super) struct CellPlacement {
    pub(super) text: String,
    pub(super) left: f32,
}

/// Splits `text` into grapheme clusters and assigns each a cell-aligned `left`
/// edge, returning `(placements, total_cells)`. A `width >= 2` cluster consumes
/// 2 cells; a `width == 0` cluster (lone combining mark) consumes 0 and merges
/// into the previous placement's text.
pub(super) fn layout_preedit_cells(
    text: &str,
    cell_w_logical: f32,
    origin_x: f32,
) -> (Vec<CellPlacement>, u32) {
    let mut placements: Vec<CellPlacement> = Vec::new();
    let mut cum_cells: u32 = 0;
    for cluster in text.graphemes(true) {
        let cells = clamp_cluster_cells(cluster);
        if cells == 0 {
            match placements.last_mut() {
                Some(last) => last.text.push_str(cluster),
                // NOTE: a leading zero-width cluster (a combining mark with no
                // base) has nothing to merge into; render it at the origin so
                // the overlay still shows every typed character. It consumes no
                // cell.
                None => placements.push(CellPlacement {
                    text: cluster.to_string(),
                    left: origin_x,
                }),
            }
            continue;
        }
        placements.push(CellPlacement {
            text: cluster.to_string(),
            left: origin_x + cum_cells as f32 * cell_w_logical,
        });
        cum_cells += cells;
    }
    (placements, cum_cells)
}

/// Cell width of one grapheme cluster, clamped to the renderer's `runs_to_cells`
/// rule: `width >= 2` is 2 cells, `width == 0` is 0, otherwise 1.
fn clamp_cluster_cells(cluster: &str) -> u32 {
    match UnicodeWidthStr::width(cluster) {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(advance: f32, line_height: f32) -> CellMetrics {
        CellMetrics {
            advance_phys: advance,
            line_height_phys: line_height,
            ascent_phys: 12.0,
            descent_phys: 4.0,
            underline_position_phys: -2.0,
            underline_thickness_phys: 1.0,
            max_overflow_phys: 0.0,
        }
    }

    fn host_inputs(top_left_logical: Vec2, size_logical: Vec2, scale: f32) -> (Vec2, Vec2) {
        let size_phys = size_logical * scale;
        let top_left_phys = top_left_logical * scale;
        let center_phys = top_left_phys + 0.5 * size_phys;
        (center_phys, size_phys)
    }

    #[test]
    fn places_overlay_at_cursor_row() {
        let (translation_phys, size_phys) = host_inputs(Vec2::ZERO, Vec2::new(800.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (3, 5),
            &metrics(10.0, 16.0),
            0.0,
            1.0,
        );
        assert_eq!(pos.y, 80.0);
        assert_eq!(pos.x, 30.0);
    }

    #[test]
    fn divides_by_scale_factor_for_logical_px() {
        let (translation_phys, size_phys) =
            host_inputs(Vec2::new(100.0, 0.0), Vec2::new(800.0, 600.0), 2.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (0, 0),
            &metrics(10.0, 16.0),
            0.0,
            2.0,
        );
        assert_eq!(pos.x, 100.0);
        assert_eq!(pos.y, 0.0);
    }

    #[test]
    fn floors_subpixel_cell_pitch() {
        let (translation_phys, size_phys) = host_inputs(Vec2::ZERO, Vec2::new(800.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (10, 1),
            &metrics(10.4, 16.4),
            0.0,
            1.0,
        );
        assert_eq!(pos.x, 100.0);
        assert_eq!(pos.y, 16.0);
    }

    #[test]
    fn clamps_right_when_overlay_overflows() {
        let (translation_phys, size_phys) = host_inputs(Vec2::ZERO, Vec2::new(800.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (78, 0),
            &metrics(10.0, 16.0),
            100.0,
            1.0,
        );
        assert_eq!(pos.x, 700.0);
    }

    #[test]
    fn clamps_left_when_composition_too_wide_to_fit() {
        let (translation_phys, size_phys) = host_inputs(Vec2::ZERO, Vec2::new(80.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (7, 0),
            &metrics(10.0, 16.0),
            200.0,
            1.0,
        );
        assert_eq!(pos.x, 0.0);
    }

    #[test]
    fn host_translated_to_window_offset_does_not_leak_into_cell_origin() {
        let (translation_phys, size_phys) =
            host_inputs(Vec2::new(10.0, 20.0), Vec2::new(1200.0, 640.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (5, 3),
            &metrics(10.0, 16.0),
            0.0,
            1.0,
        );
        assert_eq!(pos.x, 60.0);
        assert_eq!(pos.y, 68.0);
    }

    #[test]
    fn caret_cell_offsets_ascii_caret_at_start() {
        assert_eq!(caret_cell_offsets("hello", (0, 0)), (0.0, 0.0));
    }

    #[test]
    fn caret_cell_offsets_ascii_caret_at_end() {
        assert_eq!(caret_cell_offsets("hello", (5, 5)), (5.0, 5.0));
    }

    #[test]
    fn caret_cell_offsets_ascii_clause_range() {
        assert_eq!(caret_cell_offsets("hello", (2, 4)), (2.0, 4.0));
    }

    #[test]
    fn caret_cell_offsets_cjk_fullwidth() {
        assert_eq!(caret_cell_offsets("にほん", (0, 9)), (0.0, 6.0));
    }

    #[test]
    fn caret_cell_offsets_mixed_ascii_and_cjk() {
        assert_eq!(caret_cell_offsets("aあ", (0, 4)), (0.0, 3.0));
        assert_eq!(caret_cell_offsets("aあ", (1, 4)), (1.0, 3.0));
    }

    #[test]
    fn layout_preedit_cells_ascii() {
        let (cells, total) = layout_preedit_cells("abc", 10.0, 100.0);
        assert_eq!(total, 3);
        let lefts: Vec<f32> = cells.iter().map(|c| c.left).collect();
        assert_eq!(lefts, vec![100.0, 110.0, 120.0]);
        let texts: Vec<&str> = cells.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
    }

    #[test]
    fn layout_preedit_cells_fullwidth_cjk_consumes_two_cells_each() {
        let (cells, total) = layout_preedit_cells("あい", 10.0, 0.0);
        assert_eq!(total, 4);
        let lefts: Vec<f32> = cells.iter().map(|c| c.left).collect();
        assert_eq!(lefts, vec![0.0, 20.0]);
    }

    #[test]
    fn layout_preedit_cells_mixed_ascii_and_cjk() {
        let (cells, total) = layout_preedit_cells("aあb", 10.0, 0.0);
        assert_eq!(total, 4);
        let lefts: Vec<f32> = cells.iter().map(|c| c.left).collect();
        assert_eq!(lefts, vec![0.0, 10.0, 30.0]);
    }

    #[test]
    fn layout_preedit_cells_combining_mark_merges_into_previous() {
        let (cells, total) = layout_preedit_cells("e\u{0301}", 10.0, 0.0);
        assert_eq!(total, 1);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].text, "e\u{0301}");
        assert_eq!(cells[0].left, 0.0);
    }

    #[test]
    fn layout_preedit_cells_empty() {
        let (cells, total) = layout_preedit_cells("", 10.0, 0.0);
        assert_eq!(total, 0);
        assert!(cells.is_empty());
    }

    #[test]
    fn layout_preedit_cells_leading_combining_mark_is_not_dropped() {
        let (cells, total) = layout_preedit_cells("\u{0301}ab", 10.0, 0.0);
        let texts: Vec<&str> = cells.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["\u{0301}", "a", "b"]);
        assert_eq!(total, 2);
        assert_eq!(cells[0].left, 0.0);
        assert_eq!(cells[1].left, 0.0);
        assert_eq!(cells[2].left, 10.0);
    }

    #[test]
    fn caret_offset_matches_clamped_cell_layout_for_wide_cluster() {
        let family = "👨\u{200d}👩\u{200d}👧";
        let (_, total_cells) = layout_preedit_cells(family, 1.0, 0.0);
        let (_, end_cells) = caret_cell_offsets(family, (0, family.len()));
        assert_eq!(end_cells, total_cells as f32);
        assert!(
            end_cells <= 2.0,
            "a single grapheme cluster must clamp to at most 2 cells, got {end_cells}"
        );
    }
}
