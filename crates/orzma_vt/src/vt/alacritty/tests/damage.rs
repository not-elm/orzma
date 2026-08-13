//! Interpret and damage-staging tests.

use super::*;

#[test]
fn empty_chunk_is_not_a_cycle() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    assert_eq!(vt.interpret(b""), None);
}

#[test]
fn empty_chunk_leaves_staged_damage_untouched() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    vt.interpret(b"hi");
    let staged = vt.pending_damage.clone();
    assert!(
        staged.is_some(),
        "precondition: interpreting a non-empty chunk must stage damage"
    );
    assert_eq!(vt.interpret(b""), None);
    assert_eq!(
        vt.pending_damage, staged,
        "an empty chunk must neither re-read the damage tracker nor clear the staged value"
    );
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
    drain_staged(&mut vt);
    assert_eq!(vt.interpret(b"hi"), Some(Damage::Delta(vec![0].into())));
}

#[test]
fn a_multi_row_write_reports_each_dirty_row() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    assert_eq!(
        vt.interpret(b"one\r\ntwo\r\nthree"),
        Some(Damage::Delta(vec![0, 1, 2].into()))
    );
}

#[test]
fn insert_mode_reports_full_damage() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    assert_eq!(vt.interpret(b"\x1b[4h"), Some(Damage::Full));
}

#[test]
fn interpret_stages_the_damage_it_returns() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    assert_eq!(
        vt.interpret(b"one\r\ntwo\r\nthree"),
        Some(Damage::Delta(vec![0, 1, 2].into()))
    );
    assert_eq!(
        vt.pending_damage,
        Some(Damage::Delta(vec![0, 1, 2].into())),
        "the staged rows must be the ones the call returned"
    );
}

/// Asserts that a row damaged by an earlier chunk survives into the
/// staged value of a later one.
///
/// Case: two PTY chunks arrive between emits. This holds for two
/// independent reasons — alacritty keeps expanding `damage.lines`
/// until `reset_damage`, and staging merges — so it passes either
/// way and does not guard the merge on its own.
#[test]
fn staged_damage_accumulates_across_chunks() {
    let mut vt = AlacrittyVtBackend::new(80, GRID_ROWS);
    drain_staged(&mut vt);
    vt.interpret(b"a");
    vt.interpret(b"\r\n\r\nb");
    let Some(Damage::Delta(rows)) = &vt.pending_damage else {
        panic!(
            "expected staged partial damage, got {:?}",
            vt.pending_damage
        );
    };
    assert!(
        rows.contains(&0),
        "the row written by the first chunk must survive into the second cycle, got {rows:?}"
    );
    assert!(
        rows.contains(&2),
        "the row written by the second chunk must be staged, got {rows:?}"
    );
}

#[test]
fn a_fresh_vt_stages_bootstrap_full_damage() {
    // Without this, a `frames()` call that precedes the first `interpret`
    // finds nothing staged and the bootstrap paint never reaches the
    // renderer.
    assert_eq!(
        AlacrittyVtBackend::new(80, GRID_ROWS).pending_damage,
        Some(Damage::Full)
    );
}

// NOTE: `TermDamageIterator::new` truncates the trailing `display_offset`
//       entries BEFORE filtering (alacritty `term/mod.rs:194-198`). Once
//       `display_offset >= screen_lines` the whole slice is gone, so the
//       iterator yields nothing even though `Term::damage` always damages
//       the cursor — which is what makes an empty `Delta` reachable.
//       alacritty's own `damage_public_usage` (`term/mod.rs:3025-3036`)
//       asserts the same empty `Partial`.
#[test]
fn a_viewport_fully_in_scrollback_stages_empty_damage() {
    let mut vt = vt_with_history(usize::from(GRID_ROWS) + SEEDED_HISTORY_ROWS);
    vt.scroll(Scroll::Delta(i32::from(GRID_ROWS)));
    assert_eq!(
        vt.display_offset(),
        DisplayOffset(u32::from(GRID_ROWS)),
        "precondition: the viewport must sit entirely in scrollback"
    );
    drain_staged(&mut vt);
    assert_eq!(
        vt.interpret(b"\x1b[H"),
        Some(Damage::Delta(DamageRows::default()))
    );
    assert_eq!(
        vt.pending_damage,
        Some(Damage::Delta(DamageRows::default()))
    );
}
