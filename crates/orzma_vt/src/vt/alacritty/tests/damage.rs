//! Interpret and damage-reporting tests.

use super::*;

#[test]
fn empty_chunk_is_not_a_cycle() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    assert_eq!(vt.interpret(b""), None);
}

#[test]
fn the_first_interpret_on_a_fresh_vt_reports_full() {
    // Whatever the chunk contains: the bootstrap `Full` outranks it.
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    assert_eq!(vt.interpret(b"x"), Some(Damage::Full));
}

#[test]
fn a_single_row_write_reports_one_dirty_row() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    vt.term.reset_damage();
    assert_eq!(vt.interpret(b"hi"), Some(Damage::Delta(vec![0].into())));
}

#[test]
fn a_multi_row_write_reports_each_dirty_row() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    vt.term.reset_damage();
    assert_eq!(
        vt.interpret(b"one\r\ntwo\r\nthree"),
        Some(Damage::Delta(vec![0, 1, 2].into()))
    );
}

#[test]
fn insert_mode_reports_full_damage() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    vt.term.reset_damage();
    assert_eq!(vt.interpret(b"\x1b[4h"), Some(Damage::Full));
}

/// Asserts that each `interpret` reports only the damage its own
/// chunk produced: the tracker is reset after every read.
///
/// Case: two PTY chunks arrive between one emit and the next.
#[test]
fn a_second_interpret_reports_only_new_damage() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    vt.term.reset_damage();
    vt.interpret(b"one\r\ntwo\r\nthree");
    assert_eq!(vt.interpret(b"x"), Some(Damage::Delta(vec![2].into())));
}

// NOTE: `TermDamageIterator::new` truncates the trailing `display_offset`
//       entries BEFORE filtering (alacritty `term/mod.rs:194-198`). Once
//       `display_offset >= screen_lines` the whole slice is gone, so the
//       iterator yields nothing even though `Term::damage` always damages
//       the cursor — which is what makes an empty `Delta` reachable.
//       alacritty's own `damage_public_usage` (`term/mod.rs:3025-3036`)
//       asserts the same empty `Partial`.
#[test]
fn a_viewport_fully_in_scrollback_reports_empty_damage() {
    let mut vt = vt_with_history(usize::from(GRID_ROWS) + SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(i32::from(GRID_ROWS)));
    assert_eq!(
        vt.display_offset(),
        DisplayOffset(u32::from(GRID_ROWS)),
        "precondition: the viewport must sit entirely in scrollback"
    );
    assert_eq!(
        vt.interpret(b"\x1b[H"),
        Some(Damage::Delta(DamageRows::default()))
    );
}
