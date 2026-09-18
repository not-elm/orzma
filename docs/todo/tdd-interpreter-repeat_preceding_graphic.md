# Test cases: Executor::repeat_preceding_graphic

`REP`（`CSI Pn b`）を受け持つ `Executor::repeat_preceding_graphic`（`crates/orzma_vt/src/interpreter.rs:576`）のテストケース一覧。
ECMA-48 § 8.3.103 を主な出典とし、繰り返した文字が通常の文字と同じ扱いを受ける部分は VT510 の SS2 / DECAWM / IRM / SGR の記述から導いた。
ECMA-48 が定めていない部分（制御機能を挟んだ場合、結合文字、捨てられた文字、RIS）は、骨組みで追加した doc コメントを出典にしている。

- 引用の検証: **23/23**（manual 13 件、リポジトリの doc コメント 10 件）。
- Phase 4 の結果: 21 件で承認。確認なしで確定したのは TC-01 の 1 件。API 改定の提案はなし。
- 承認後、著者の判断で TC-A11（`CSI 0 b`）を追加した（2026-09-17）。manual とは食い違う契約なので、仕様から導いたケースではない（付録「仕様の矛盾」）。
- この一覧は仕様から導いたものを自分で検証しただけで、他の誰かがレビューしたものではない。使う前に引用を 1〜2 件確かめてほしい。

**前提となる木の状態。** 対象メソッドと、それが呼ぶ `DeviceState::preceding_graphic` / `DeviceState::print_graphic` / `Screen::translate` / `Screen::print_graphic` は、この一覧を作る前に骨組みとして追加した（コミット `f31b3134`）。
記憶の更新と RIS での消去はコミット `13e3cc47` で実装し、下の 22 件は `crates/orzma_vt/src/interpreter/tests/repeat.rs` に書き写して、すべて通っている。
コードレビューで、一覧の外のテストを `repeat.rs` に 6 件足した（private marker 付きの REP、行幅を超える回数の REP、ASCII の文字を出してから線画を指定した後の REP、主画面へ戻った後の REP、カーソルが動かない REP の damage、スクロールバック中の REP）。

## テストケース一覧

優先度は High → Medium → Low の順に並べた。High までで、仕様が明示している振る舞いはすべて揃う。

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `a_repeat_prints_the_preceding_character_the_requested_number_of_times` | C1 | High |
| TC-02 | `a_repeat_without_a_parameter_prints_the_preceding_character_once` | C4 | High |
| TC-03 | `a_repeat_with_a_parameter_of_one_prints_the_preceding_character_once` | C1 | High |
| TC-04 | `a_repeat_of_a_space_prints_spaces` | C2 | High |
| TC-05 | `a_repeat_counts_a_single_shifted_character_as_the_character_to_repeat` | C1, C7, PG1 | High |
| TC-A1 | `a_repeat_before_any_graphic_character_prints_nothing` | RP3 | High |
| TC-A2 | `a_reset_clears_the_preceding_character` | RS | High |
| TC-A11 | `a_repeat_with_a_parameter_of_zero_prints_the_preceding_character_once` | RC（著者判断） | High |
| TC-06 | `a_repeat_at_the_right_border_wraps_onto_the_next_line` | C1, C8 | Medium |
| TC-07 | `a_repeat_that_wraps_on_the_last_row_scrolls_the_page_up` | C8 | Medium |
| TC-08 | `a_repeat_at_the_right_border_without_autowrap_overwrites_the_last_column` | C9 | Medium |
| TC-09 | `a_repeat_in_insert_mode_shifts_the_rest_of_the_row_for_each_repetition` | C10, C1 | Medium |
| TC-10 | `a_repeat_uses_the_rendition_selected_after_the_preceding_character` | C11, RP1 | Medium |
| TC-11 | `a_repeat_leaves_a_pending_single_shift_for_the_next_character` | RP2, C7 | Medium |
| TC-A3 | `a_repeat_after_a_line_feed_repeats_the_character_before_it` | PG2 | Medium |
| TC-A4 | `a_second_repeat_repeats_the_same_character_again` | PG2 | Medium |
| TC-A5 | `a_repeat_after_a_combining_mark_repeats_the_base_character_alone` | SPG2, DP | Medium |
| TC-A6 | `a_repeat_after_a_character_set_change_repeats_the_glyph_first_printed` | PG1 | Medium |
| TC-A7 | `a_repeat_after_a_dropped_wide_character_repeats_that_character` | SPG1, DP | Medium |
| TC-A8 | `a_repeat_after_a_soft_reset_repeats_the_character_before_it` | PG2 | Low |
| TC-A9 | `a_repeat_after_switching_screens_repeats_the_character_printed_on_the_other_screen` | PG2 | Low |
| TC-A10 | `a_repeat_prints_inside_the_open_hyperlink` | RP1 | Low |

Source タグ — **C1**: ECMA-48.pdf p.69 L3198-3200（直前の図形文字を n 回繰り返す）／**C2**: 同 L3198-3199（SPACE を含む）／**C4**: 同 L3197（Pn の既定値 1）／**C7**: vt510.pdf p.64 L2437-2438（SS2 は次の図形文字 1 つ）／**C8**: vt510.pdf p.129 L4592-4594（DECAWM set）／**C9**: 同 L4595-4596（DECAWM reset）／**C10**: vt510.pdf p.319 L9185-9186（IRM set）／**C11**: vt510.pdf p.342 L9688（SGR は新しい文字に適用）／**PG1・PG2・DP・RS**: `device.rs` の doc コメント／**RP1・RP2・RP3**: `interpreter.rs` の doc コメント／**SPG1・SPG2**: `screen.rs` の doc コメント。全文は付録。

TC-01〜TC-11 は manual の記述を少なくとも 1 つ根拠に持つ。TC-A1〜TC-A10 は manual に根拠がなく、ECMA-48 が「定めない」とした部分（C3）や VT の manual が扱わない結合文字・ハイパーリンクを、リポジトリの doc コメントが決めた契約として固定するケース。TC-A11 は manual と食い違う契約を、著者の判断で固定するケース。

テストコードは新しいファイル `crates/orzma_vt/src/interpreter/tests/repeat.rs` に置き、`crates/orzma_vt/src/interpreter/tests.rs` の `mod private_modes;` と `mod reset;` の間に `mod repeat;` を足す。
REP を扱う既存のテストモジュールはない。
`interpret_wide`（20 列 × 3 行）、`interpret_sized`（列数を指定、3 行）、`glyph_at`、`cell_at`、`first_row_glyphs` を使う。

ファイルの先頭はこうする。

```rust
//! Tests for `REP`, which prints the preceding graphic character again.

use super::*;
use crate::screen::cell::CellWidth;
```

## TC-01 — REP は直前の文字をパラメータの回数だけ繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `CSI 3 b` |
| Expect | 列 0〜3 が `A` **[C1]** ／ 列 4 が空白 **[C1]** |

列 4 の空白は「n 回」を超えて出していないことを確かめる。4 列の `interpret` では列 4 が存在しないので、20 列を使う。

```rust
/// Asserts that `REP` prints the preceding character as many more times
/// as its parameter asks.
///
/// Case: an ncurses program in a non-UTF-8 locale draws a run of four
/// identical letters by printing one letter and repeating it three times.
#[test]
fn a_repeat_prints_the_preceding_character_the_requested_number_of_times() {
    let device = interpret_wide(b"A\x1b[3b");
    for column in 0..4 {
        assert_eq!(glyph_at(&device, 0, column), 'A');
    }
    assert_eq!(glyph_at(&device, 0, 4), ' ');
}
```

## TC-02 — パラメータを省略した REP は 1 回繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `CSI b` |
| Expect | 列 0〜1 が `A` **[C4]** ／ 列 2 が空白 **[C4]** |

省略を 0 回と読む実装を防ぐ。

```rust
/// Asserts that `REP` with its parameter omitted prints the preceding
/// character once more.
///
/// Case: a program prints one character and sends `CSI b` with no count.
#[test]
fn a_repeat_without_a_parameter_prints_the_preceding_character_once() {
    let device = interpret_wide(b"A\x1b[b");
    assert_eq!(glyph_at(&device, 0, 0), 'A');
    assert_eq!(glyph_at(&device, 0, 1), 'A');
    assert_eq!(glyph_at(&device, 0, 2), ' ');
}
```

## TC-03 — パラメータ 1 の REP は 1 回繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `CSI 1 b` |
| Expect | 列 0〜1 が `A` **[C1]** ／ 列 2 が空白 **[C1]** |

数値パラメータの展開（省略・1・2 以上・0）のうちの明示の 1。0 は仕様と食い違うため、著者の判断で足した TC-A11 が扱う（付録「仕様の矛盾」）。

```rust
/// Asserts that `REP` with an explicit count of one prints the preceding
/// character once more.
///
/// Case: vttest's REP screen prints a plus sign and follows it with an
/// explicit count of one.
#[test]
fn a_repeat_with_a_parameter_of_one_prints_the_preceding_character_once() {
    let device = interpret_wide(b"A\x1b[1b");
    assert_eq!(glyph_at(&device, 0, 0), 'A');
    assert_eq!(glyph_at(&device, 0, 1), 'A');
    assert_eq!(glyph_at(&device, 0, 2), ' ');
}
```

## TC-04 — SPACE も繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列）; 行 0 に `xxxxxx`; `CSI 1 G` で列 0 へ |
| Act | ` ` `CSI 2 b` |
| Expect | 列 0〜2 が空白 **[C2]** ／ 列 3 は `x` のまま **[C2]** |

空白を「何も表示していない」と扱って記憶しない実装を防ぐ。既存の `x` の上に書くので、空白が実際に置かれたかどうかが見える。

```rust
/// Asserts that `REP` repeats a space like any other graphic character,
/// overwriting the cells it lands on.
///
/// Case: a program blanks the start of a line of text by printing one
/// space over it and repeating that space.
#[test]
fn a_repeat_of_a_space_prints_spaces() {
    let device = interpret_wide(b"xxxxxx\x1b[1G \x1b[2b");
    for column in 0..3 {
        assert_eq!(glyph_at(&device, 0, column), ' ');
    }
    assert_eq!(glyph_at(&device, 0, 3), 'x');
}
```

## TC-05 — SS2 で出した文字を 1 つの図形文字として繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列）; `ESC * 0` で G2 に DEC Special Graphic を指定 |
| Act | `ESC N` `q` `CSI 2 b` |
| Expect | 列 1〜2 が列 0 と同じ文字 **[C1]** **[C7]** **[PG1]** |

C1 の「represented by one or more bit combinations」により、SS2 と `q` で 1 つの図形文字になる。変換前の `q` を記憶して GL（ASCII）で変換し直す実装では、列 1〜2 が `q` になって列 0 と一致しない。

```rust
/// Asserts that `REP` after a single-shifted character repeats the glyph
/// the single shift selected.
///
/// Case: a program designates line drawing into G2, shows one
/// line-drawing character through `SS2`, and repeats it to extend a rule.
#[test]
fn a_repeat_counts_a_single_shifted_character_as_the_character_to_repeat() {
    let device = interpret_wide(b"\x1b*0\x1bNq\x1b[2b");
    let shifted = glyph_at(&device, 0, 0);
    assert_eq!(glyph_at(&device, 0, 1), shifted);
    assert_eq!(glyph_at(&device, 0, 2), shifted);
}
```

## TC-A1 — 図形文字を表示する前の REP は何もしない

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `CSI 3 b` |
| Expect | 行 0 がすべて空白 **[RP3]** ／ カーソルは列 0 **[RP3]** |

ECMA-48 は、前に文字がない場合を定めていない。記憶の初期値を空白などにして、空白を繰り返したりカーソルを進めたりする実装を防ぐ。

```rust
/// Asserts that `REP` before any graphic character has been printed
/// prints nothing and leaves the cursor where it was.
///
/// Case: `CSI 3 b` is the first thing a freshly started terminal
/// receives.
#[test]
fn a_repeat_before_any_graphic_character_prints_nothing() {
    let device = interpret_wide(b"\x1b[3b");
    assert!(first_row_glyphs(&device).iter().all(|&glyph| glyph == ' '));
    assert_eq!(device.active_screen().cursor_column(), GridColumn(0));
}
```

## TC-A2 — RIS で記憶が消える

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `ESC c` `CSI 3 b` |
| Expect | 行 0 がすべて空白 **[RS]** |

RIS は画面を消してカーソルを原点に戻すので、記憶が残っていれば列 0〜2 に `A` が出る。

```rust
/// Asserts that `RIS` clears the preceding graphic character, so a `REP`
/// after it prints nothing.
///
/// Case: a program prints a character, the terminal is hard reset, and a
/// stale `REP` arrives afterwards.
#[test]
fn a_reset_clears_the_preceding_character() {
    let device = interpret_wide(b"A\x1bc\x1b[3b");
    assert!(first_row_glyphs(&device).iter().all(|&glyph| glyph == ' '));
}
```

## TC-A11 — パラメータ 0 の REP は 1 回繰り返す（著者判断で追加）

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `CSI 0 b` |
| Expect | 列 0〜1 が `A` **[RC]** ／ 列 2 が空白 **[RC]** |

ECMA-48 § 5.4.2 f（C5）と C1 を合わせると 0 回になるが、`repeat_count` の doc（RC）と spec の D8 は 0 を 1 とする。vttest の `tst_REP` の 1 行目が `CSI 0 b` を送るので、著者の判断で追加した。REP の腕が `repeat_count` を通らずに値をそのまま使う実装を防ぐ。

```rust
/// Asserts that `REP` with an explicit count of zero prints the preceding
/// character once more rather than repeating it zero times.
///
/// Case: vttest's REP screen prints a plus sign on its first row and
/// follows it with an explicit count of zero.
#[test]
fn a_repeat_with_a_parameter_of_zero_prints_the_preceding_character_once() {
    let device = interpret_wide(b"A\x1b[0b");
    assert_eq!(glyph_at(&device, 0, 0), 'A');
    assert_eq!(glyph_at(&device, 0, 1), 'A');
    assert_eq!(glyph_at(&device, 0, 2), ' ');
}
```

## TC-06 — 右端をまたぐ REP は次の行へ折り返す

| | |
| - | - |
| Setup | `interpret_sized(10, …)`; `CSI 9 G` で列 8 へ |
| Act | `A` `CSI 3 b` |
| Expect | 行 0 の列 8〜9 が `A` **[C1]** **[C8]** ／ 行 1 の列 0〜1 が `A` **[C8]** |

繰り返した文字も、右端で受けた図形文字として DECAWM に従う。セルを直接書いてカーソルを右端で止める実装を防ぐ。

```rust
/// Asserts that a `REP` crossing the right border continues at the start
/// of the next line while autowrap is set.
///
/// Case: a curses program repeats a character across the right border of
/// a ten-column terminal with autowrap on.
#[test]
fn a_repeat_at_the_right_border_wraps_onto_the_next_line() {
    let (device, _) = interpret_sized(10, b"\x1b[9GA\x1b[3b");
    assert_eq!(glyph_at(&device, 0, 8), 'A');
    assert_eq!(glyph_at(&device, 0, 9), 'A');
    assert_eq!(glyph_at(&device, 1, 0), 'A');
    assert_eq!(glyph_at(&device, 1, 1), 'A');
}
```

## TC-07 — 最終行で折り返す REP は画面をスクロールする

| | |
| - | - |
| Setup | `interpret_sized(10, …)`（3 行）; `CSI 999;9 H` で最終行の列 8 へ |
| Act | `A` `CSI 3 b` |
| Expect | 行 1 の列 8〜9 が `A` **[C8]** ／ 行 2 の列 0〜1 が `A` **[C8]** |

`CSI 999;9 H` は最終行に丸められる。最終行に出した `A` がスクロールで 1 つ上の行へ移ることで、折り返しがスクロールを伴ったことを確かめる。

```rust
/// Asserts that a `REP` wrapping on the bottom row scrolls the page up
/// while autowrap is set.
///
/// Case: a program draws a divider along the bottom row of a full screen
/// with `REP`, and the divider runs past the right border.
#[test]
fn a_repeat_that_wraps_on_the_last_row_scrolls_the_page_up() {
    let (device, _) = interpret_sized(10, b"\x1b[999;9HA\x1b[3b");
    assert_eq!(glyph_at(&device, 1, 8), 'A');
    assert_eq!(glyph_at(&device, 1, 9), 'A');
    assert_eq!(glyph_at(&device, 2, 0), 'A');
    assert_eq!(glyph_at(&device, 2, 1), 'A');
}
```

## TC-08 — autowrap が無効な REP は最終列を上書きする

| | |
| - | - |
| Setup | `interpret_sized(10, …)`; 行 0 を `x` で埋める; `CSI ?7 l`; `CSI 9 G` で列 8 へ |
| Act | `A` `CSI 3 b` |
| Expect | 行 0 の列 8〜9 が `A` **[C9]** ／ 行 1 がすべて空白 **[C9]** |

行 0 を先に `x` で埋めるので、列 9 が `A` に置き換わったことが見える。10 個の `x` で遅延折り返しが立つが、`CSI ?7 l` と `CSI 9 G` が解除する。

```rust
/// Asserts that a `REP` reaching the right border replaces the last
/// column instead of wrapping while autowrap is reset.
///
/// Case: a status-line program with autowrap turned off repeats a
/// character past the right border.
#[test]
fn a_repeat_at_the_right_border_without_autowrap_overwrites_the_last_column() {
    let (device, _) = interpret_sized(10, b"xxxxxxxxxx\x1b[?7l\x1b[9GA\x1b[3b");
    assert_eq!(glyph_at(&device, 0, 8), 'A');
    assert_eq!(glyph_at(&device, 0, 9), 'A');
    for column in 0..10 {
        assert_eq!(glyph_at(&device, 1, column), ' ');
    }
}
```

## TC-09 — 挿入モードの REP は繰り返すたびに行を右へずらす

| | |
| - | - |
| Setup | `interpret_sized(10, …)`; 行 0 に `abcdef`; `CSI 1 G`; `CSI 4 h` |
| Act | `X` `CSI 2 b` |
| Expect | 行 0 の列 0〜8 が `XXXabcdef` **[C10]** **[C1]** |

繰り返し分の 2 文字も、それぞれ新しい文字として既存の文字を右へずらす。挿入を 1 回分しか行わない実装では `XXXbcdef` になる。

```rust
/// Asserts that each character a `REP` prints in insert mode shifts the
/// rest of the row right.
///
/// Case: an editor in insert mode inserts a run of identical characters
/// before existing text.
#[test]
fn a_repeat_in_insert_mode_shifts_the_rest_of_the_row_for_each_repetition() {
    let (device, _) = interpret_sized(10, b"abcdef\x1b[1G\x1b[4hX\x1b[2b");
    let row: String = first_row_glyphs(&device).into_iter().take(9).collect();
    assert_eq!(row, "XXXabcdef");
}
```

## TC-10 — REP は REP の時点の SGR を使う

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `CSI 1 m` `CSI 2 b` |
| Expect | 列 0 は太字でない **[C11]** ／ 列 1〜2 は太字 **[C11]** **[RP1]** |

記憶するのは文字だけで、属性は REP の時点の pen を使う。元のセルを属性ごと複製する実装を防ぐ。

```rust
/// Asserts that the characters a `REP` prints take the rendition selected
/// after the preceding character, not the one it was printed with.
///
/// Case: a program prints a character, switches to bold, and then repeats
/// the character.
#[test]
fn a_repeat_uses_the_rendition_selected_after_the_preceding_character() {
    let device = interpret_wide(b"A\x1b[1m\x1b[2b");
    assert!(!cell_at(&device, 0, 0).style.contains(Style::BOLD));
    assert!(cell_at(&device, 0, 1).style.contains(Style::BOLD));
    assert!(cell_at(&device, 0, 2).style.contains(Style::BOLD));
}
```

## TC-11 — 保留中の SS2 は REP ではなく次の実際の文字に効く

| | |
| - | - |
| Setup | `interpret_wide`（20 列）; `ESC * 0`; `ESC N` `q` で列 0 に基準のグリフを出す |
| Act | `q` `ESC N` `CSI 2 b` `q` |
| Expect | 列 2〜3 が `q` **[RP2]** ／ 列 4 が列 0 と同じ文字 **[C7]** |

変換前の文字を記憶して REP のたびに `print` し直す実装では、1 回目の繰り返しが SS2 を使ってしまい、列 2 が線の文字、列 4 が `q` になる。

```rust
/// Asserts that a `REP` leaves a pending single shift for the next
/// character received rather than spending it on a repetition.
///
/// Case: a program leaves a single shift pending, sends `REP`, and then
/// prints the character the shift was meant for.
#[test]
fn a_repeat_leaves_a_pending_single_shift_for_the_next_character() {
    let device = interpret_wide(b"\x1b*0\x1bNqq\x1bN\x1b[2bq");
    assert_eq!(glyph_at(&device, 0, 2), 'q');
    assert_eq!(glyph_at(&device, 0, 3), 'q');
    assert_eq!(glyph_at(&device, 0, 4), glyph_at(&device, 0, 0));
}
```

## TC-A3 — 改行を挟んだ REP も前の文字を繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `CR` `LF` `CSI 2 b` |
| Expect | 行 1 の列 0〜1 が `A` **[PG2]** |

ECMA-48 では、REP の直前が制御機能のときの結果は定められていない（C3）。「RIS まで保持する」という決定を固定する。制御機能で記憶を消す実装（xterm 方式）を防ぐ。

```rust
/// Asserts that a `REP` after a line break still repeats the character
/// printed before it, rather than printing nothing.
///
/// Case: a program prints a character, moves to the next line with CR
/// LF, and sends `REP` there.
#[test]
fn a_repeat_after_a_line_feed_repeats_the_character_before_it() {
    let device = interpret_wide(b"A\r\n\x1b[2b");
    assert_eq!(glyph_at(&device, 1, 0), 'A');
    assert_eq!(glyph_at(&device, 1, 1), 'A');
}
```

## TC-A4 — REP の直後の REP も同じ文字を繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `CSI b` `CSI b` |
| Expect | 列 0〜2 が `A` **[PG2]** |

vttest の `tst_REP` は 2 回目の REP を無視する前提（xterm と同じ）だが、この設計ではそうならないことを固定する。REP 自身が記憶を消す実装を防ぐ。

```rust
/// Asserts that a second `REP` right after a first one repeats the same
/// character again rather than being ignored.
///
/// Case: vttest's REP screen sends a second `REP` right after the first
/// one.
#[test]
fn a_second_repeat_repeats_the_same_character_again() {
    let device = interpret_wide(b"A\x1b[b\x1b[b");
    for column in 0..3 {
        assert_eq!(glyph_at(&device, 0, column), 'A');
    }
}
```

## TC-A5 — 結合文字の後の REP は基底文字だけを繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `e` `U+0301` `CSI 2 b` |
| Expect | 列 0 は `e` に `U+0301` が付く **[SPG2]** ／ 列 1〜2 は結合文字なしの `e` **[DP]** |

幅 0 の文字で記憶を更新する実装では、結合文字が列 1 の直前のセル（列 0）に積み重なり、列 1〜2 は空白のままになる。

```rust
/// Asserts that a `REP` after a combining mark repeats the base character
/// without the mark.
///
/// Case: a program prints an e with a combining acute accent and then
/// repeats it.
#[test]
fn a_repeat_after_a_combining_mark_repeats_the_base_character_alone() {
    let device = interpret_wide("e\u{301}\x1b[2b".as_bytes());
    let base = cell_at(&device, 0, 0);
    assert_eq!(base.c, 'e');
    assert_eq!(base.marks(), ['\u{301}']);
    for column in 1..3 {
        let repeated = cell_at(&device, 0, column);
        assert_eq!(repeated.c, 'e');
        assert!(repeated.marks().is_empty());
    }
}
```

## TC-A6 — 文字セットを切り替えた後の REP は最初に表示したグリフを繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `ESC ( 0` `q` `ESC ( B` `CSI 2 b` |
| Expect | 列 1〜2 が列 0 と同じ文字 **[PG1]** |

変換前の `q` を記憶して REP の時点の G0（ASCII）で変換し直す実装では、列 1〜2 が `q` になる。

```rust
/// Asserts that a `REP` after a character set change repeats the glyph
/// first shown, not the byte mapped through the new set.
///
/// Case: a program prints one line-drawing character, designates ASCII
/// back into G0, and then repeats.
#[test]
fn a_repeat_after_a_character_set_change_repeats_the_glyph_first_printed() {
    let device = interpret_wide(b"\x1b(0q\x1b(B\x1b[2b");
    let first = glyph_at(&device, 0, 0);
    assert_eq!(glyph_at(&device, 0, 1), first);
    assert_eq!(glyph_at(&device, 0, 2), first);
}
```

## TC-A7 — 捨てられた全角文字も REP の対象になる

| | |
| - | - |
| Setup | `interpret_sized(10, …)`; `CSI ?7 l`; `CSI 10 G` で最終列へ |
| Act | `中` `CSI 1 G` `CSI b` |
| Expect | 列 9 は空白のまま **[SPG1]** ／ 列 0〜1 が全角の `中` **[DP]** |

autowrap が無効な最終列の全角文字は捨てられる（SPG1）。セルに置けたときだけ記憶する実装では、REP が何も出さない。

```rust
/// Asserts that a wide character the screen drops still becomes the
/// character a later `REP` repeats.
///
/// Case: with autowrap off, a wide character arrives on the last column
/// of a ten-column screen and is dropped, and the program moves to the
/// first column and sends `REP`.
#[test]
fn a_repeat_after_a_dropped_wide_character_repeats_that_character() {
    let (device, _) = interpret_sized(10, "\x1b[?7l\x1b[10G中\x1b[1G\x1b[b".as_bytes());
    assert_eq!(glyph_at(&device, 0, 9), ' ');
    let body = cell_at(&device, 0, 0);
    assert_eq!(body.c, '中');
    assert_eq!(body.width, CellWidth::Wide);
    assert_eq!(cell_at(&device, 0, 1).width, CellWidth::Spacer);
}
```

## TC-A8 — DECSTR を挟んでも REP は前の文字を繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `CSI ! p` `CSI b` |
| Expect | 列 0〜1 が `A` **[PG2]** |

DECSTR はカーソル位置を変えないので、繰り返しは列 1 に出る。soft reset でも記憶を消す実装を防ぐ。

```rust
/// Asserts that a soft reset leaves the preceding graphic character, so a
/// `REP` after it still repeats.
///
/// Case: a program prints a character, performs a soft reset, and then
/// repeats.
#[test]
fn a_repeat_after_a_soft_reset_repeats_the_character_before_it() {
    let device = interpret_wide(b"A\x1b[!p\x1b[b");
    assert_eq!(glyph_at(&device, 0, 0), 'A');
    assert_eq!(glyph_at(&device, 0, 1), 'A');
}
```

## TC-A9 — 画面を切り替えた後の REP は、もう一方の画面で出した文字を繰り返す

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `x` `CSI ?1049 h` `CSI H` `CSI 2 b` |
| Expect | 代替画面の行 0 の列 0〜1 が `x` **[PG2]** |

記憶を画面ごとに持つ実装（B 案）では、代替画面には記憶がなく、何も出ない。

```rust
/// Asserts that the preceding graphic character is shared by both
/// screens, so a `REP` on the alternate screen repeats what the primary
/// screen printed.
///
/// Case: a shell prints a character, and a full-screen program enters the
/// alternate screen and sends `REP` before printing anything there.
#[test]
fn a_repeat_after_switching_screens_repeats_the_character_printed_on_the_other_screen() {
    let device = interpret_wide(b"x\x1b[?1049h\x1b[H\x1b[2b");
    assert_eq!(device.modes().active_screen, ScreenKind::Alternate);
    assert_eq!(glyph_at(&device, 0, 0), 'x');
    assert_eq!(glyph_at(&device, 0, 1), 'x');
}
```

## TC-A10 — REP は開いているハイパーリンクの中に出る

| | |
| - | - |
| Setup | `interpret_wide`（20 列） |
| Act | `A` `OSC 8 ;; https://a.example ST` `CSI 2 b` |
| Expect | 列 0 はリンクなし **[RP1]** ／ 列 1〜2 はリンク付き **[RP1]** |

REP の時点のハイパーリンクを使う。元のセルのリンク状態を複製する実装を防ぐ。

```rust
/// Asserts that the characters a `REP` prints join the hyperlink open at
/// the time of the `REP`.
///
/// Case: a build tool prints a character, opens a hyperlink, and repeats
/// the character inside the link.
#[test]
fn a_repeat_prints_inside_the_open_hyperlink() {
    let device = interpret_wide(b"A\x1b]8;;https://a.example\x1b\\\x1b[2b");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].hyperlink_id, None);
    assert!(row[1].hyperlink_id.is_some());
    assert!(row[2].hyperlink_id.is_some());
}
```

## 付録

### 契約表

`pdftotext -layout` の抽出に対する行番号。ページは PDF のページ番号（form feed を数えたもの）。

| ID | Governs | Shape | Statement | Citation |
| --- | --- | --- | --- | --- |
| C1 | REP | unconditional | "the preceding character in the data stream, if it is a graphic character (represented by one or more bit combinations) including SPACE, is to be repeated n times, where n equals the value of Pn" | ECMA-48.pdf p.69, L3198-3200 |
| C2 | REP | conditional | "if it is a graphic character (represented by one or more bit combinations) including SPACE" | ECMA-48.pdf p.69, L3198-3199 |
| C3 | REP | conditional | "If the character preceding REP is a control function or part of a control function, the effect of REP is not defined by this Standard"（未定義なのでケースは作らない） | ECMA-48.pdf p.69, L3200-3201 |
| C4 | REP | numeric parameter | "Parameter default value: Pn = 1" | ECMA-48.pdf p.69, L3197 |
| C5 | Parameter string format | bounded value | "If the parameter sub-string consists of bit combinations 03/00 only, at least one of them must be retained to indicate the zero value of the sub-string" | ECMA-48.pdf p.26, L1023-1025 |
| C5z | ZDM | mode-dependent | "DEFAULT: A parameter value of 0 represents a default parameter value which may be different from 0" | ECMA-48.pdf p.102, L4541-4542 |
| C5l | ZDM | mode-dependent | "Control functions affected are: … REP …" | ECMA-48.pdf p.102, L4553-4554 |
| C6 | REP | unconditional | "CSI Ps b Repeat the preceding graphic character Ps times (REP)" | xterm-ctlseqs.pdf p.16, L789 |
| C7 | SS2 | unconditional | "Single shift 2 SS2 Temporarily maps the G2 character set into GL, for the next graphic character" | vt510.pdf p.64, L2437-2438 |
| C8 | DECAWM | mode-dependent | "If the DECAWM function is set, then graphic characters received when the cursor is at the right border of the page appear at the beginning of the next line. Any text on the page scrolls up if the cursor is at the end of the scrolling region" | vt510.pdf p.129, L4592-4594 |
| C9 | DECAWM | mode-dependent | "If the DECAWM function is reset, then graphic characters received when the cursor is at the right border of the page replace characters already on the page" | vt510.pdf p.129, L4595-4596 |
| C10 | IRM | mode-dependent | "If IRM mode is set, then new characters move characters in page memory to the right. Characters moved past the page's right border are lost" | vt510.pdf p.319, L9185-9186 |
| C11 | SGR | unconditional | "After you select an attribute, the terminal applies that attribute to all new characters received" | vt510.pdf p.342, L9688 |

リポジトリの doc コメント（kind 2）。すべて `grep -F` 相当の完全一致で確認した。

```
PG1 — crates/orzma_vt/src/device.rs:148-150, DeviceState::preceding_graphic
      "The graphic character `REP` repeats: the last character one or two columns wide that [`Self::print`] mapped, or `None` when it has mapped none since power-up or the last `RIS`."
      justifies: 繰り返すのは変換後のグリフで、何も表示していなければ記憶はない
PG2 — crates/orzma_vt/src/device.rs:152-153, DeviceState::preceding_graphic
      "Control functions, a soft reset, and a flip between the screens leave it as it is; only [`Self::reset`] clears it."
      justifies: 改行・REP・DECSTR・画面切り替えを挟んでも記憶が残る
DP  — crates/orzma_vt/src/device.rs:119-122, DeviceState::print
      "A mapped character one or two columns wide becomes the [`Self::preceding_graphic`], even when the screen drops it instead of placing it. A zero-width mark and a character with no width leave the preceding graphic character as it was."
      justifies: 捨てられた全角文字も記憶し、結合文字では記憶を更新しない
RS  — crates/orzma_vt/src/device.rs:203-204, DeviceState::reset
      "The preceding graphic character is cleared, so a `REP` that follows prints nothing."
      justifies: RIS の後の REP は何も出さない
RP1 — crates/orzma_vt/src/interpreter.rs:565-567, Executor::repeat_preceding_graphic
      "Prints the device's preceding graphic character `count` more times, each time at the cursor as an ordinary print shaped by the current pen, hyperlink, `IRM`, and `DECAWM`."
      justifies: 繰り返しは REP の時点の pen とハイパーリンクを使う
RP2 — crates/orzma_vt/src/interpreter.rs:569, Executor::repeat_preceding_graphic
      "A pending single shift stays pending."
      justifies: 保留中の SS2 は繰り返しで使われない
RP3 — crates/orzma_vt/src/interpreter.rs:569-571, Executor::repeat_preceding_graphic
      "Nothing is printed when the device has no preceding graphic character, and the repetitions stop at the first glyph a row refuses."
      justifies: 記憶がなければ何も出さず、カーソルも動かない
RC  — crates/orzma_vt/src/interpreter.rs:890, repeat_count
      "A repeat count parameter, where an omitted or zero value means one."
      justifies: （C5 と矛盾するため、ケースには使っていない）
SPG1 — crates/orzma_vt/src/screen.rs:215-217, Screen::print_graphic
      "A two-column glyph with one column left wraps first, leaving a filler in the last column; with autowrap reset it is dropped and the deferred wrap is disarmed."
      justifies: autowrap が無効な最終列の全角文字は捨てられ、最終列は空白のまま
SPG2 — crates/orzma_vt/src/screen.rs:212-213, Screen::print_graphic
      "a zero-width mark joins the glyph the cursor last passed and leaves the cursor alone."
      justifies: 結合文字は列 0 の `e` に付く
```

### 探索した語

| 段階 | 語 | 結果 |
| --- | --- | --- |
| 1（メソッドの doc） | `REP` / `CSI Pn b` | ECMA-48 L3194-3201 の本文。L368（目次）、L955（コード表）、L2066（索引表）の 3 件は定義ではないので除外。VT510 の 2 件（L4488 は DECARM、L5389 はマクロの Pn）も除外。VT220 は該当なし。xterm-ctlseqs L789 |
| 1 | `repeat the` / `repeat preceding` / `CSI Ps b` | VT510・VT220 には REP の定義なし（上の除外のみ） |
| 1 | `preceding graphic character` | ECMA-48 L3198-3200、xterm-ctlseqs L789 |
| 1 | `single shift` / `SS2` | VT510 L2437-2438（採用）、L9790-9800（同内容の章。VT510 の表の行を採用）、VT220 L1097（VT510 を優先） |
| 1 | `IRM` | VT510 L9185-9186（採用）。L6626-6636 は右から左への記入時の記述なので除外 |
| 1 | `DECAWM` | VT510 L4592-4596 |
| 1 | `RIS` | VT510 L9419-9421。REP の記憶に関する記述はない |
| 1 | `pen` → `SGR` | VT510 L9688 |
| 1 | `hyperlink` / `OSC 8` | `docs/references/` に該当なし |
| 2（`impl Executor` の doc） | — | 新しい語なし |
| 3（`interpreter.rs` の `//!`） | `synchronized-update` | REP とは無関係 |
| 4（名前と型から） | `graphic character` | ECMA-48 § 4.2.43 L712-714（C1 の括弧書きと同じ内容なので C1 に含めた） |
| 4 | `parameter default` / `zero value` | ECMA-48 § 5.4.2 L1022-1025、付録 F.4.2 L4533-4554 |
| 4 | `Special Graphic` / `SCS` | VT510 L9578-9623（指定子 `0` の表。振る舞いの記述ではない） |
| 4 | `combining` | 振る舞いの記述なし（xterm-ctlseqs L1850 はマウスの設定、xlib は無関係） |
| 4 | `DECSTR` | VT510 L8191。REP の記憶に関する記述はない |

### 仕様にないもの

- **RP3 の後半「the repetitions stop at the first glyph a row refuses」**
  - doc コメントの契約だが、行が文字を拒否する（`StampError`）状態をバイト列の Setup から作る方法がないので、ケースにしていない。
- **65535 回の上限**
  - `CsiParams` がパラメータを u16 に飽和させて変換することによる。REP の doc にも manual にも記述がないので、ケースにしていない。

### 仕様の矛盾

- **`CSI 0 b`（パラメータ 0）**
  - ECMA-48 § 5.4.2 f（C5）は、0 だけの部分文字列を「zero value」とする。C1 の「n equals the value of Pn」と合わせると、0 回の繰り返しになる。
  - `repeat_count` の doc（RC）は「an omitted or zero value means one」で、REP の腕はこれを使う。
  - ECMA-48 付録 F.4.2 の ZDM（C5z、C5l）は、0 を既定値とみなす側で、対象に REP を挙げている。ただしこれは第 1 版の実装向けに残されたモード。
  - 規則どおり、skill の実行ではケースを作らなかった。会話の中では「省略と 0 は 1」で合意済みで、vttest の `tst_REP` も 0 を 1 回として扱う。
  - 承認後、著者がこのケースを TC-A11 として追加すると決めた（2026-09-17）。

### API 改定

この一覧を作る前に、骨組みとして次を追加した（コミット `f31b3134`）。一覧はこの形を前提にしている。

- `Screen::print` を `Screen::translate(&mut self, c: char) -> GraphicChar` と `Screen::print_graphic(&mut self, GraphicChar, PrintOptions) -> VtResult<Option<DamageSpan>>` に分けた。`Screen::print` は `#[cfg(test)]` になった。
- `DeviceState` に `preceding_graphic: Option<GraphicChar>` を追加した。あわせて `pub fn preceding_graphic(&self) -> Option<GraphicChar>` と `pub fn print_graphic(&mut self, glyph: GraphicChar) -> VtResult<Option<DamageSpan>>` を追加した。
- `Executor::repeat_preceding_graphic(&mut self, count: u16)` と、`csi_dispatch` の `(None, [], b'b')` の腕を追加した。

この実行での API 改定の提案はない。

コードレビューと simplify で、変換済みの文字とその `GlyphClass` を組にした `ClassifiedGlyph`（`crates/orzma_vt/src/screen/cell.rs`）を足し、`Screen::print_graphic` と `DeviceState::print_graphic` はこれを受け取るようにした。`DeviceState` の記憶もこの型にしたので、文字の幅は変換の直後に 1 回だけ引く。振る舞いは変わらない。

その後、テスト専用だった `Screen::print(char, PrintOptions)` を削除し、`Screen::print_graphic` を `Screen::print(ClassifiedGlyph, PrintOptions)` に改名した。この文書の他の箇所にある `Screen::print_graphic` は、今の `Screen::print` を指す。`Screen` のテストは補助関数 `classified(c)` で引数を作る。制御文字は `ClassifiedGlyph` にならないので、`Screen` 層の「制御文字を無視する」テストは削除した（`DeviceState::print` を通る `interpreter/tests/printing.rs` の DEL・UTF-8 ST のテストと、`cell.rs` の `a_control_character_has_no_class` が同じ契約を持つ）。`DeviceState::print_graphic` の名前はそのままである。

### docs/todo との衝突

- なし。`docs/todo/vt-conformance-remaining.md` にあった REP の項目（「`csi_dispatch` に腕が無い」）は、実装後にコミット `727ff77e` で一覧から外した。
