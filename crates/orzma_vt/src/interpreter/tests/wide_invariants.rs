//! Property test: every row, history included, keeps the wide-pair
//! invariant under any mix of printing, editing, and resizing.

use super::*;
use proptest::prelude::*;

/// One step of terminal input.
#[derive(Debug, Clone)]
enum TestOp {
    Ascii(u8),
    Cjk,
    Mark,
    Cha(u8),
    Ich(u8),
    Dch(u8),
    Ech(u8),
    El(u8),
    Ed(u8),
    Resize(u16, u16),
    Irm(bool),
    Decawm(bool),
    Cr,
    Lf,
    Bs,
}

fn op_strategy() -> impl Strategy<Value = TestOp> {
    prop_oneof![
        (b'a'..=b'z').prop_map(TestOp::Ascii),
        Just(TestOp::Cjk),
        Just(TestOp::Mark),
        (1u8..8).prop_map(TestOp::Cha),
        (1u8..4).prop_map(TestOp::Ich),
        (1u8..4).prop_map(TestOp::Dch),
        (1u8..4).prop_map(TestOp::Ech),
        (0u8..3).prop_map(TestOp::El),
        (0u8..3).prop_map(TestOp::Ed),
        (2u16..9, 1u16..4).prop_map(|(cols, rows)| TestOp::Resize(cols, rows)),
        any::<bool>().prop_map(TestOp::Irm),
        any::<bool>().prop_map(TestOp::Decawm),
        Just(TestOp::Cr),
        Just(TestOp::Lf),
        Just(TestOp::Bs),
    ]
}

fn apply(vt: &mut OrzmaVt, op: &TestOp) {
    match op {
        TestOp::Ascii(byte) => {
            vt.interpret(&[*byte]);
        }
        TestOp::Cjk => {
            vt.interpret("あ".as_bytes());
        }
        TestOp::Mark => {
            vt.interpret("\u{0301}".as_bytes());
        }
        TestOp::Cha(n) => {
            vt.interpret(format!("\x1b[{n}G").as_bytes());
        }
        TestOp::Ich(n) => {
            vt.interpret(format!("\x1b[{n}@").as_bytes());
        }
        TestOp::Dch(n) => {
            vt.interpret(format!("\x1b[{n}P").as_bytes());
        }
        TestOp::Ech(n) => {
            vt.interpret(format!("\x1b[{n}X").as_bytes());
        }
        TestOp::El(n) => {
            vt.interpret(format!("\x1b[{n}K").as_bytes());
        }
        TestOp::Ed(n) => {
            vt.interpret(format!("\x1b[{n}J").as_bytes());
        }
        TestOp::Resize(cols, rows) => {
            let _ = vt.resize(GridSize {
                cols: *cols,
                rows: *rows,
            });
        }
        TestOp::Irm(on) => {
            let mode = if *on { 'h' } else { 'l' };
            vt.interpret(format!("\x1b[4{mode}").as_bytes());
        }
        TestOp::Decawm(on) => {
            let mode = if *on { 'h' } else { 'l' };
            vt.interpret(format!("\x1b[?7{mode}").as_bytes());
        }
        TestOp::Cr => {
            vt.interpret(b"\r");
        }
        TestOp::Lf => {
            vt.interpret(b"\n");
        }
        TestOp::Bs => {
            vt.interpret(b"\x08");
        }
    }
}

/// The first row, top of history to bottom of screen, whose wide pairs
/// are broken, as a `GridLine`.
fn first_broken_row(vt: &OrzmaVt) -> Option<GridLine> {
    let grid = vt.device.active_screen().grid();
    let history = i32::try_from(grid.history_len()).expect("history fits an i32");
    let rows = i32::from(grid.size().rows);
    (-history..rows)
        .map(GridLine)
        .find(|line| !grid.row(*line).wide_pairs_intact())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Asserts that after every operation, every row of the screen and
    /// its history holds an intact wide-pair invariant.
    ///
    /// Case: a program prints Japanese text, accents, and ASCII in any
    /// order while editing the row with insert, delete, and erase
    /// sequences, toggling insert mode and autowrap, moving the cursor
    /// by carriage return, line feed, backspace, and column addressing,
    /// and resizing the window.
    #[test]
    fn every_row_keeps_the_wide_pair_invariant(
        ops in prop::collection::vec(op_strategy(), 1..64)
    ) {
        let mut vt = OrzmaVt::new(GridSize { cols: 6, rows: 3 }, 8);
        for (step, op) in ops.iter().enumerate() {
            apply(&mut vt, op);
            let broken = first_broken_row(&vt);
            prop_assert!(broken.is_none(), "row {broken:?} broke at step {step} after {op:?}");
        }
    }
}
