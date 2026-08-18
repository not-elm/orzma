//! Tests for [`OldOrzmaVt`]: shared fixtures, the damage-staging tests,
//! plus one child module per exercised concern.

use super::*;
use crate::schema::{GridColumn, GridLine, Row, Run};

mod frame;
mod interpret;

/// Builds a wrapper whose bootstrap damage and backend accumulator are
/// both consumed, so a test observes only what its own calls stage.
fn clean_vt() -> OldOrzmaVt<AlacrittyVtBackend> {
    let mut vt = OldOrzmaVt::new(80, 24);
    vt.interpret(b"\x1b[H");
    vt.pending_damage = None;
    vt
}

/// Builds a wrapper with `history_rows` scrollback lines and its
/// bootstrap damage consumed; the 24-row viewport absorbs the first 23
/// newlines before history starts growing.
fn vt_with_history(history_rows: usize) -> OldOrzmaVt<AlacrittyVtBackend> {
    let mut vt = OldOrzmaVt::new(80, 24);
    let seed: Vec<u8> = (0..history_rows + 23)
        .flat_map(|i| format!("l{i}\r\n").into_bytes())
        .collect();
    vt.interpret(&seed);
    vt.pending_damage = None;
    vt
}

fn start_simple(vt: &mut OldOrzmaVt<AlacrittyVtBackend>, x: u16, line: i32) -> bool {
    vt.start_selection(
        GridPoint {
            line: GridLine(line),
            column: GridColumn(x),
        },
        CellSide::Left,
        SelectionKind::Simple,
    )
    .unwrap()
}

fn row_text(row: &Row<Run>) -> String {
    row.iter().map(|run| run.text.as_str()).collect()
}

/// Asserts that a fresh wrapper stages the bootstrap full repaint.
///
/// Case: the first emit precedes any PTY output from a shell that
/// stays silent.
#[test]
fn a_fresh_vt_stages_bootstrap_full_damage() {
    let vt = OldOrzmaVt::<AlacrittyVtBackend>::new(80, 24);
    assert_eq!(vt.pending_damage, Some(Damage::Full));
}

/// Asserts that `interpret` stages the damage it classified.
///
/// Case: ordinary shell output arrives between two emits.
#[test]
fn interpret_stages_the_damage_it_classified() {
    let mut vt = clean_vt();
    assert_eq!(
        vt.interpret(b"one\r\ntwo\r\nthree"),
        Some(DamageVerdict::ManyRows { rows: 3 })
    );
    assert_eq!(vt.pending_damage, Some(Damage::Delta(vec![0, 1, 2].into())));
}

/// Asserts that damage staged by an earlier chunk survives into the
/// staged value of a later one.
///
/// Case: two PTY chunks arrive between one emit and the next.
#[test]
fn staged_damage_accumulates_across_chunks() {
    let mut vt = clean_vt();
    vt.interpret(b"a");
    vt.interpret(b"\r\n\r\nb");
    let Some(Damage::Delta(rows)) = &vt.pending_damage else {
        panic!(
            "expected staged partial damage, got {:?}",
            vt.pending_damage
        );
    };
    assert!(rows.contains(&0), "row from the first chunk, got {rows:?}");
    assert!(rows.contains(&2), "row from the second chunk, got {rows:?}");
}

/// Asserts that an empty chunk neither reports a cycle nor disturbs
/// the staged value.
///
/// Case: a zero-length PTY read lands between two real chunks.
#[test]
fn an_empty_chunk_leaves_staged_damage_untouched() {
    let mut vt = clean_vt();
    vt.interpret(b"hi");
    let staged = vt.pending_damage.clone();
    assert!(
        staged.is_some(),
        "precondition: a real chunk must stage damage"
    );
    assert_eq!(vt.interpret(b""), None);
    assert_eq!(vt.pending_damage, staged);
}

/// Asserts that a viewport-moving scroll stages full damage and
/// reports the move.
///
/// Case: the user wheels into scrollback on an otherwise idle
/// terminal.
#[test]
fn a_moving_scroll_stages_full_damage() {
    let mut vt = vt_with_history(17);
    assert!(vt.scroll(Scroll::Delta(3)));
    assert_eq!(vt.pending_damage, Some(Damage::Full));
}

/// Asserts that a selection change stages full damage and reports the
/// change.
///
/// Case: a mouse press anchors a selection on an otherwise idle
/// terminal.
#[test]
fn a_selection_change_stages_full_damage() {
    let mut vt = clean_vt();
    assert!(start_simple(&mut vt, 0, 0));
    assert_eq!(vt.pending_damage, Some(Damage::Full));
}

/// Asserts that no-op operations preserve the staged value exactly.
///
/// Case: clamped scrolls, stray selection ops after an alt-screen
/// wipe, an idempotent vi request, and a same-size resize all arrive
/// while damage from earlier output is still awaiting its emit.
#[test]
fn no_op_operations_preserve_staged_damage() {
    let mut vt = clean_vt();
    vt.pending_damage = Some(Damage::Delta(vec![0].into()));
    let staged = vt.pending_damage.clone();
    assert!(!vt.scroll(Scroll::Delta(0)));
    assert!(
        !vt.update_selection(
            GridPoint {
                line: GridLine(0),
                column: GridColumn(2),
            },
            CellSide::Right
        )
        .unwrap()
    );
    assert!(!vt.clear_selection().unwrap());
    assert!(!vt.switch_vi_mode(ViModeSwitch::Exit).unwrap());
    assert!(!vt.resize(80, 24));
    assert_eq!(vt.pending_damage, staged);
}
