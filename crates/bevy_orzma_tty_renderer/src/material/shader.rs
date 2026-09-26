//! Tests that pin the structure of the terminal material's WGSL shader.

/// Asserts that the shader's cursor helper consults the wide-right-half
/// flag for both halves of a wide pair, and that a bar cursor moves to
/// the body cell when parked on a wide glyph's right half.
///
/// Case: a block cursor sits on a Japanese character, either on its
/// body or parked on its right half, and a bar cursor is parked on the
/// right half.
#[test]
fn wgsl_cursor_covers_both_halves_of_a_wide_glyph() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    let body = wgsl_fn_body(src, "cursor_covers");
    assert!(body.contains("col == params.cursor_pos.x + 1u && wide_right_half_at(row, col)"));
    assert!(body.contains(
        "col + 1u == params.cursor_pos.x && wide_right_half_at(row, params.cursor_pos.x)"
    ));
    assert!(
        wgsl_fn_body(src, "wide_right_half_at")
            .contains("is_wide_right_half(cells[idx].style_flags)")
    );
    assert!(wgsl_fn_body(src, "is_wide_right_half").contains("STYLE_WIDE_RIGHT_HALF"));
    assert!(wgsl_fn_body(src, "paint_cursor").contains("stroked_cursor_covers(row, col)"));
    let covers = wgsl_fn_body(src, "stroked_cursor_covers");
    assert!(covers.contains("if cursor_shape() == CURSOR_SHAPE_BAR"));
    assert!(covers.contains("return bar_covers(row, col);"));
    assert!(covers.contains("return cursor_covers(row, col);"));
    assert!(wgsl_fn_body(src, "bar_covers").contains("col == cursor_span_left()"));
}

/// The text of WGSL function `name`, from the end of its name to its
/// closing brace in column zero, under LF and CRLF line endings alike.
fn wgsl_fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    src.split(&format!("fn {name}("))
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("the shader defines the function")
}

/// Asserts that a function body ends at its own closing brace under
/// CRLF line endings rather than running on into the next function.
///
/// Case: a Windows checkout converts the shader to CRLF before the
/// tests embed it.
#[test]
fn wgsl_fn_body_ends_at_the_closing_brace_under_crlf() {
    let src =
        "fn first(\r\n) {\r\n    if x {\r\n    }\r\n}\r\nfn second() {\r\n    MARKER\r\n}\r\n";
    assert!(!wgsl_fn_body(src, "first").contains("MARKER"));
    assert!(wgsl_fn_body(src, "second").contains("MARKER"));
}

/// Asserts that the shader applies concealment as the last stage of
/// color resolution, after reverse video and dim.
///
/// Case: a program prints concealed text that is also reverse-video
/// or faint.
#[test]
fn wgsl_concealment_is_the_last_color_resolution_stage() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    assert!(wgsl_fn_body(src, "conceal").contains("STYLE_HIDDEN"));
    assert!(!wgsl_fn_body(src, "resolve_visible_colors").contains("STYLE_HIDDEN"));
    assert!(
        wgsl_fn_body(src, "resolve_cell_colors")
            .contains("conceal(cell, resolve_visible_colors(cell))")
    );
}

/// Asserts that a concealed glyph takes the tinted color its ground
/// is painted in rather than the untinted cell background.
///
/// Case: a program prints concealed text on a colored background in
/// a pane that then loses focus and takes the inactive-pane tint.
#[test]
fn wgsl_concealment_follows_the_inactive_pane_tint() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    assert!(wgsl_fn_body(src, "conceal").contains("CellColors(tint_bg(colors.bg), colors.bg)"));
}

/// Asserts that reverse video materializes the transparent default
/// background through the shared helper rather than inline.
///
/// Case: a program prints a reverse-video cell on the default
/// background, and its glyph takes the colour that background paints.
#[test]
fn wgsl_reverse_video_materializes_the_default_background_through_the_helper() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    let visible = wgsl_fn_body(src, "resolve_visible_colors");
    assert!(visible.contains("reverse_video(colors)"));
    assert!(!visible.contains("bg_padding_color"));
    let reverse = wgsl_fn_body(src, "reverse_video");
    assert!(reverse.contains("materialize_default_bg("));
    assert!(!reverse.contains("bg_padding_color"));
    assert!(wgsl_fn_body(src, "materialize_default_bg").contains("params.bg_padding_color"));
}

/// Asserts that both paint paths resolve cell colors through the
/// block-cursor override, leaving the plain resolution to the left
/// neighbour's overdraw alone, and that the override conceals last.
///
/// Case: a block cursor sits on a cell in the last column, so the
/// grid path and the right-strip path both paint it.
#[test]
fn wgsl_block_cursor_is_resolved_into_the_cell_colors() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    assert!(wgsl_fn_body(src, "paint_grid_cell").contains("resolve_painted_colors("));
    assert!(wgsl_fn_body(src, "paint_right_strip").contains("resolve_painted_colors("));
    assert_eq!(
        src.matches("resolve_cell_colors(").count(),
        2,
        "only the definition and paint_left_overdraw name the plain resolution"
    );
    let painted = wgsl_fn_body(src, "resolve_painted_colors");
    assert!(painted.contains("block_cursor_covers(row, col)"));
    assert!(painted.contains("conceal("));
    assert!(!painted.contains("STYLE_HIDDEN"));
    assert!(wgsl_fn_body(src, "block_cursor_covers").contains("cursor_covers(row, col)"));
}

/// Asserts that a hollow caret is left to the stroke painter rather
/// than filling the cell it sits on.
///
/// Case: a block caret sits on a cell in a pane that loses focus, so
/// the caret is drawn as an outline.
#[test]
fn wgsl_a_hollow_block_caret_does_not_fill_the_cell() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    assert!(wgsl_fn_body(src, "block_cursor_covers").contains("!cursor_is_hollow()"));
    assert!(wgsl_fn_body(src, "cursor_is_hollow").contains("CURSOR_HOLLOW"));
}

/// Asserts that every stroked cursor — the bar, the underline and the
/// hollow outline — takes the guarded fill color from the cell's
/// colors before concealment, compared against the tinted ground, and
/// that no pixel inversion is left in the shader.
///
/// Case: an underline cursor sits on a concealed cell in an inactive
/// pane, which also draws its caret hollow.
#[test]
fn wgsl_cursor_strips_take_the_guarded_fill_color() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    let painter = wgsl_fn_body(src, "paint_cursor");
    assert!(painter.contains("!on_cursor_stroke(col, in_cell_px)"));
    assert!(painter.contains("return cursor_stroke_fill(cell);"));
    let stroke = wgsl_fn_body(src, "on_cursor_stroke");
    assert!(stroke.contains("on_hollow_outline(col, in_cell_px)"));
    assert!(stroke.contains("CURSOR_SHAPE_UNDERLINE"));
    assert!(stroke.contains("CURSOR_SHAPE_BAR"));
    let fill = wgsl_fn_body(src, "cursor_stroke_fill");
    assert!(fill.contains("let visible = resolve_visible_colors(cell);"));
    assert!(fill.contains("let ground = tint_bg(materialize_default_bg(visible.bg));"));
    assert!(fill.contains("return guarded_fill(cursor_fill(visible.fg), ground);"));
    assert!(wgsl_fn_body(src, "cursor_fill").contains("params.cursor_packed"));
    assert!(!src.contains("1.0 - base.rgb"));
}

/// Asserts that the block cursor passes its fill through the contrast
/// guard, which falls back to the default foreground or background
/// against the cell's ground.
///
/// Case: a light-theme editor leaves the cursor on a white cell whose
/// foreground is also white, and a theme sets a cursor color close to
/// a reverse-video cell's ground.
#[test]
fn wgsl_cursor_fill_is_guarded_against_the_ground() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    assert!(
        wgsl_fn_body(src, "resolve_painted_colors")
            .contains("colors = under_block_cursor(colors);")
    );
    let block = wgsl_fn_body(src, "under_block_cursor");
    assert!(block.contains("let ground = materialize_default_bg(colors.bg);"));
    assert!(block.contains("let fill = guarded_fill(cursor_fill(colors.fg), ground);"));
    let guard = wgsl_fn_body(src, "guarded_fill");
    assert!(guard.contains("contrast_ratio("));
    assert!(guard.contains("MIN_CURSOR_CONTRAST"));
    assert!(guard.contains("params.default_fg_packed"));
    assert!(guard.contains("materialize_default_bg("));
    assert!(src.contains("const MIN_CURSOR_CONTRAST: f32 = 1.5;"));
}

/// Asserts that under the block cursor a glyph that does not stand
/// out from its own ground is painted in the guarded fill rather
/// than in the ground.
///
/// Case: a color picker draws a swatch as a full block whose
/// foreground and background are the same color, and the block
/// cursor lands on it.
#[test]
fn wgsl_block_cursor_paints_a_glyph_that_melts_into_its_ground_in_the_fill() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    let block = wgsl_fn_body(src, "under_block_cursor");
    assert!(block.contains(
        "let glyph_melts = contrast_ratio(colors.fg.rgb, ground.rgb) < MIN_CURSOR_CONTRAST;"
    ));
    assert!(block.contains("CellColors(select(ground, fill, glyph_melts), fill)"));
}

/// Asserts that the hollow caret suppresses the inner edges of a
/// wide pair, so an unfocused caret over a CJK character is one
/// outline rather than two boxes.
///
/// Case: the user switches panes while the caret sits on a CJK
/// character.
#[test]
fn the_hollow_caret_spans_a_wide_pair_without_an_inner_seam() {
    let src = include_str!("../shaders/terminal_ui_material.wgsl");
    assert!(src.contains("fn cursor_span_left("));
    assert!(src.contains("fn cursor_span_right("));
    let outline = wgsl_fn_body(src, "on_hollow_outline");
    assert!(outline.contains("col == cursor_span_left()"));
    assert!(outline.contains("col == cursor_span_right()"));
}
