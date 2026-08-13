//! Tests for [`OrzmaVt`]'s damage staging over the alacritty backend.

use super::*;
use crate::schema::{CellSide, ViewportPoint};

/// Builds a wrapper whose bootstrap damage and backend accumulator are
/// both consumed, so a test observes only what its own calls stage.
fn clean_vt() -> OrzmaVt<AlacrittyVtBackend> {
    let mut vt = OrzmaVt::new(80, 24);
    vt.interpret(b"\x1b[H");
    vt.pending_damage = None;
    vt
}

fn vt_with_history() -> OrzmaVt<AlacrittyVtBackend> {
    let mut vt = OrzmaVt::new(80, 24);
    let seed: Vec<u8> = (0..40)
        .flat_map(|i| format!("l{i}\r\n").into_bytes())
        .collect();
    vt.interpret(&seed);
    vt.pending_damage = None;
    vt
}

fn start_simple(vt: &mut OrzmaVt<AlacrittyVtBackend>, x: u16, y: i16) -> bool {
    vt.apply_selection(SelectionOp::StartAt {
        cell: ViewportPoint { row: y, column: x },
        side: CellSide::Left,
        kind: SelectionKind::Simple,
    })
    .unwrap()
}

/// Asserts that a fresh wrapper stages the bootstrap full repaint.
///
/// Case: the first emit can precede any PTY output — a spawned shell
/// that stays silent still needs its whole grid painted once.
#[test]
fn a_fresh_vt_stages_bootstrap_full_damage() {
    let vt = OrzmaVt::<AlacrittyVtBackend>::new(80, 24);
    assert_eq!(vt.pending_damage, Some(Damage::Full));
}

/// Asserts that `interpret` stages the damage it classified.
///
/// Case: ordinary shell output between two emits — the verdict drives
/// the flush decision while the staged rows drive the repaint.
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
/// Case: two PTY chunks arrive between emits, and the emit must
/// repaint what both of them dirtied.
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
    let mut vt = vt_with_history();
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
        !vt.apply_selection(SelectionOp::UpdateTo {
            cell: ViewportPoint { row: 0, column: 2 },
            side: CellSide::Right,
        })
        .unwrap()
    );
    assert!(!vt.apply_selection(SelectionOp::Clear).unwrap());
    assert!(!vt.switch_vi_mode(ViModeSwitch::Exit).unwrap());
    assert!(!vt.resize(80, 24));
    assert_eq!(vt.pending_damage, staged);
}
