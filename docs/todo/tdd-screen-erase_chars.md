# Test cases: Screen::erase_chars

ECH（`CSI Pn X`）を実装する `Screen::erase_chars` と、そのディスパッチが負うテストケース。
`docs/references/` の vt510 / vt220 / ECMA-48 から導出し、citation は 14/15 を機械検証済み
（棄却された1件は付録に記録）。

**Phase 4 の合意**: signature は提案どおり。ケースは Codex のレビューを受けて 5 件から
**8 件**へ拡張（Screen 層 6 件 + interpreter 層 2 件）。

> **このドキュメントの Rust は現状の木に対してコンパイルできない。**
> `Screen::erase_chars` はまだ存在せず、`csi_dispatch` にも `b'X'` の分岐が無い。
> 下記 signature を前提に書かれている。
>
> **Rust はビルド検証されていない。** このスキルは書いたコードをコンパイルしない
> （そもそも対象が存在しないため不可能）。貼って動くものではなく、転記する提案として読むこと。
>
> レビュー節は無い。一覧は仕様由来かつ自己検証で、その後 Codex CLI に独立レビューを
> 依頼して指摘を反映した。信頼する前に citation を1〜2件、実際の PDF で抜き取り確認すること。

## 前提とする signature

```rust
// crates/orzma_vt/src/screen.rs — `/// Erasure.` impl、erase_in_line の直後
pub fn erase_chars(&mut self, count: u16) -> Option<DamageSpan>

// crates/orzma_vt/src/interpreter.rs — csi_dispatch、IL の手前
// ECH
(None, b'X') => {
    let damage = self
        .device
        .active_screen_mut()
        .erase_chars(repeat_count(params.value(0)));
    self.stage(damage);
}
```

`count: u16`（正規化済み）は IL / DL / SU / SD と同じ層分け。`Pn` の復号と既定値は
interpreter 側の `repeat_count` が持つ。**前提条件: `count >= 1`。**

## テストケース一覧

### Screen 層 — `crates/orzma_vt/src/screen/tests/erase_chars.rs`（新規）

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `erasing_characters_clears_the_span_without_shifting_the_rest` | C1, C5, C6, C12, Damage | High |
| TC-02 | `erasing_past_the_last_column_stops_at_the_row_end` | C1, C12, Damage | High |
| TC-03 | `erasing_characters_outside_the_scrolling_region_still_clears` | C3, Damage | High |
| TC-04 | `erasing_characters_below_a_scrolled_viewport_reports_no_damage` | C1, Damage | Medium |
| TC-05 | `erasing_characters_leaves_the_cursor_on_its_column` | C10 | Medium |
| TC-06 | `erasing_characters_clears_the_attributes_and_takes_the_pen_background` | C11, C12, BCE | High |
| TC-09 | `erasing_characters_is_a_no_op_under_pending_wrap` | WRAP | High |

### interpreter 層 — `crates/orzma_vt/src/interpreter/tests/erase.rs`（既存に追記）

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-07 | `the_erase_character_sequence_clears_the_span_at_the_cursor` | C1, C5, C10 | High |
| TC-08 | `an_omitted_or_zero_erase_character_count_erases_one_cell` | C4, C9 | High |

> **TC-07 / TC-08 が無いと、この一覧は今回のバグを検出できない。** TC-01〜TC-06 は
> `Screen::erase_chars` を直接呼ぶだけなので、`csi_dispatch` に `b'X'` の分岐が無くても
> 全て通る。実際に起きた不具合は「`CSI X` が `_ => {}` に落ちる」ことだった。
> バイト列から検証する層が要る。

Source タグ — **C1**: vt510 PDF p.309 L9028（カーソル位置から右へ消す）／**C3**: vt510 PDF
p.309 L9029-9030（スクロールマージンの内外を問わない）／**C4**: vt510 PDF p.309 L9039-9040
（`Pn` は消す文字数、0 か 1 なら1文字、既定1）／**C5**: vt220 PDF p.36 L1750（カーソル位置と
続く Pn-1 文字）／**C6**: vt220 PDF p.36 L1751-1752（属性は normal、行の再配置は起きない）／
**C9**: ECMA-48 PDF p.55 L2475（既定 `Pn = 1`）／**C10**: vt220 PDF p.36 L1744（消去でカーソル
位置は変わらない）／**C11**: vt220 PDF p.36 L1744-1745（文字の消去はその文字属性も消す）／
**C12**: vt220 PDF p.36 L1743（消去は画面上の他の文字に影響しない）／**BCE**: kind-2、
`Pen::erase_cell`／**Damage**: スキルの Damage 決定表。全エントリは付録の契約表。

> **ページ番号は PDF のページ番号**（`\f` を数えたもの）であり、紙面に印刷された
> ページ番号ではない。ECMA-48 の PDF p.55 は印刷ページでは 41。

`tests.rs` の `mod` 宣言に `mod erase_chars;` を `mod display_offset;` と
`mod erase_in_display;` の間（アルファベット順）に足す。ファイル冒頭は他のテストモジュールと
同じく `//!` 一行と `use super::*;`。**TC-06 は `Style` を名指すので
`use crate::screen::grid::run::Style;` も要る**（`Style` は `super::*` の経路に無い）。
ヘルパは `screen()`（4x3）と `tall_screen()`（4x4）。interpreter 層は既存の
`interpret(b"...")`（4x3 のグリッド）を使う。

---

## TC-01 — カーソル位置から count セルだけ消し、右側は左詰めされない

| | |
| - | - |
| Setup | `screen()`（4x3）; `print` で `'a','b','c'`; 列3に `'d'` を直接置く; カーソルを `GridColumn(1)` |
| Act | `screen.erase_chars(2)` |
| Expect | 列0は `'a'` のまま **[C12]** ／ 列1・列2が erase セル **[C1][C5]** ／ 列3は `'d'` のまま左詰めされない **[C6]** ／ `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

列3の非移動が要点。ECH を DCH と取り違えた実装（消して左詰め）はここだけで落ちる。
カーソルを列1に置くのは、左に手つかずのセルを1つ残して「カーソル位置から右」を
両側から挟むため。列3を `print` ではなく直接代入で置くのは、4桁の画面に4文字 `print`
すると deferred wrap が張られ、ECH と無関係な状態がセットアップに混入するのを避けるため
（`Screen::print` は `column + 1 < cols` のときだけ列を進め、そうでなければ
`pending_wrap` を立てる）。

```rust
/// Asserts that erasing characters clears exactly the span at the
/// cursor, leaving the columns before it and the columns after it
/// where they were.
///
/// Case: an application overwrites a two-character field in the
/// middle of a line and clears the old value first.
#[test]
fn erasing_characters_clears_the_span_without_shifting_the_rest() {
    let mut screen = screen();
    for c in ['a', 'b', 'c'] {
        screen.print(c);
    }
    screen.grid[ScreenLine(0)][3].c = 'd';
    screen.state.column = GridColumn(1);
    let damage = screen.erase_chars(2);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][2].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'd');
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-02 — count が行末を越えたら行末で止まり、次の行に及ばない

| | |
| - | - |
| Setup | `screen()`（4x3）; `print` で `'a','b','c'`; 列3に `'d'`; 行1の列0に `'e'`; カーソルを `GridColumn(2)` |
| Act | `screen.erase_chars(10)` と `screen.erase_chars(u16::MAX)` を別々の画面で |
| Expect | 列1は `'b'` のまま **[C12]** ／ 列2・列3が erase セル **[C1]** ／ 行1の列0は `'e'` のまま **[C12]** ／ `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

**行末で止まることを明言したマニュアル文は無い。** C1 は「カーソル位置から右へ」、C5 は
「カーソル位置と続く Pn-1 文字」と述べるだけで、行に残っている数より多く要求されたときの
振る舞いを規定していない。ここで固定するのは **xterm 互換の契約**であり、仕様から論理的に
導いたものではない — 実装時にその旨を production 側の doc に書くこと。C12「消去は画面上の
他の文字に影響しない」が、要求範囲を越えて次行を潰す実装を否定する側の根拠になる。

`u16::MAX` を併せて回すのは `start + count` を `saturating_add` せずに書いた実装を
落とすため。`10` では `u16` は溢れないので、`10` だけでは境界を突けない。

```rust
/// Asserts that a count running past the last column stops at the
/// row end rather than reaching the row below, however large it is.
///
/// Case: an application clears the tail of a line by asking for
/// more characters than the row has left.
#[test]
fn erasing_past_the_last_column_stops_at_the_row_end() {
    for count in [10, u16::MAX] {
        let mut screen = screen();
        for c in ['a', 'b', 'c'] {
            screen.print(c);
        }
        screen.grid[ScreenLine(0)][3].c = 'd';
        screen.grid[ScreenLine(1)][0].c = 'e';
        screen.state.column = GridColumn(2);
        let damage = screen.erase_chars(count);
        assert_eq!(screen.grid[ScreenLine(0)][1].c, 'b');
        assert_eq!(screen.grid[ScreenLine(0)][2].c, ' ');
        assert_eq!(screen.grid[ScreenLine(0)][3].c, ' ');
        assert_eq!(screen.grid[ScreenLine(1)][0].c, 'e');
        assert_eq!(
            damage,
            Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
        );
    }
}
```

## TC-03 — スクロール領域の外にカーソルがあっても消去する

| | |
| - | - |
| Setup | `tall_screen()`（4x4）; `set_scroll_region(Some(2), Some(3))` で領域を `ScreenLine(1)..=ScreenLine(2)` に; **その後**カーソルを `ScreenLine(3)` / `GridColumn(0)` へ; `ScreenLine(3)` の列0〜2に `'x','y','z'` |
| Act | `screen.erase_chars(2)` |
| Expect | `ScreenLine(3)` の列0・列1が erase セル **[C3]** ／ 同行の列2は `'z'` のまま **[C3]** ／ `Some(DamageSpan::rows(ViewportLine(3), ViewportLine(3)))` **[Damage]** |

> **基数に注意。** `set_scroll_region` の引数は **1-based**、`ScreenLine` は **0-based**。
> `Margins::resolve` が `ScreenLine(top - 1)` / `ScreenLine(bottom - 1)` に変換する
> （`crates/orzma_vt/src/screen/margins.rs:141-142`）。したがって下端 `Some(3)` は
> `ScreenLine(2)` であり、カーソルの `ScreenLine(3)` とは**別の行**。`tall_screen()` は
> 4 行なので `ScreenLine(3)` は最終行、下端マージンの1行下で領域外になる。
> 既存の `a_resolved_region_reaches_the_scroll_span` が
> `set_scroll_region(Some(1), Some(3))` → `ScreenLine(0)..=ScreenLine(2)` を固定している。

C3 が「inside or outside」と明言している唯一の条件。IL / DL / SU / SD は領域境界で動作を
変えるので、その隣に実装される ECH が同じ境界チェックを流用してしまう誤りを狙う。
カーソルを領域設定の**後**に置くのは、`set_scroll_region` が適用時にカーソルを home に
座らせるため（`set_scroll_region.rs` の `applying_a_region_seats_the_cursor_at_home` が
その挙動を固定している）。先に置くと領域設定に上書きされてセットアップが成立しない。

```rust
/// Asserts that a cursor outside the scrolling region erases all
/// the same, because ECH ignores the margins.
///
/// Case: an application reserves a status line below its scrolling
/// region and clears a field on it.
#[test]
fn erasing_characters_outside_the_scrolling_region_still_clears() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.state.line = ScreenLine(3);
    screen.state.column = GridColumn(0);
    screen.grid[ScreenLine(3)][0].c = 'x';
    screen.grid[ScreenLine(3)][1].c = 'y';
    screen.grid[ScreenLine(3)][2].c = 'z';
    let damage = screen.erase_chars(2);
    assert_eq!(screen.grid[ScreenLine(3)][0].c, ' ');
    assert_eq!(screen.grid[ScreenLine(3)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(3)][2].c, 'z');
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(3), ViewportLine(3)))
    );
}
```

## TC-04 — 対象行がビューポート外なら damage を報告しない

| | |
| - | - |
| Setup | `screen()`; 行2で `line_feed` を3回して履歴を3行作る; `viewport.offset = DisplayOffset(3)`; カーソルを `ScreenLine(0)` / `GridColumn(0)`; そのセルに `'a'` |
| Act | `screen.erase_chars(1)` |
| Expect | 列0が erase セルになる **[C1]** ／ `None` を返す **[Damage]** |

Damage 決定表の「行は変わったが全て viewport 外 → `None`」の行。消去そのものは起きるのに
報告だけが無い、という組み合わせを固定する。`damage_span`（`screen.rs:939-948`）は
`screen_line + offset >= rows` で `None` を返すので、3 行の画面に `offset = 3` なら
`ScreenLine(0)` でも外に出る。

`print.rs:112` の `a_write_scrolled_out_of_the_window_reports_no_damage` が**同一の setup**で
同じ分岐を `print` 経由で既に固定している。分岐そのものは新規ではないので、TC-04 が足すのは
「ECH の戻り値もその分岐を通る」という配線の確認。重複と判断するなら落としてよい。
（`erase_in_display.rs:89` の `a_span_running_past_the_viewport_is_clamped_to_its_last_row`
は部分クリップの話で、こことは別の分岐。）

```rust
/// Asserts that an erase on a row the viewport no longer shows
/// reports no damage while still clearing the cells.
///
/// Case: the user has scrolled back into history when a background
/// program clears a field on the live screen.
#[test]
fn erasing_characters_below_a_scrolled_viewport_reports_no_damage() {
    let mut screen = screen();
    for _ in 0..3 {
        screen.state.line = ScreenLine(2);
        screen.line_feed();
    }
    screen.viewport.offset = DisplayOffset(3);
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(0);
    screen.grid[ScreenLine(0)][0].c = 'a';
    let damage = screen.erase_chars(1);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
    assert_eq!(damage, None);
}
```

## TC-05 — ECH の後もカーソルは同じ列に留まる

| | |
| - | - |
| Setup | `screen()`; `print` で `'a','b','c'`; カーソルを `GridColumn(1)` |
| Act | `screen.erase_chars(2)` |
| Expect | カーソル列は `GridColumn(1)` のまま **[C10]** ／ カーソル行は `ScreenLine(0)` のまま **[C10]** |

ECH は `print` の隣に実装される可能性が高く、`print` は書き込みのたびに列を進める。
その形を写して消去のたびに列を進めてしまう誤りは現実的で、この表明が無いと
TC-01〜TC-04 のどれも気づかない（いずれもカーソルを見ていない）。

典拠は vt220 §4.12 の平文（PDF p.36 L1744）"The cursor position does not change when
erasing characters or lines."。表の中の C6b（同じ内容が表組みで割れているもの）は機械検証を
通らないため使わない — 経緯は付録「仕様にないもの」を参照。

```rust
/// Asserts that the cursor stays where it was after an erase,
/// which is what separates ECH from a write.
///
/// Case: an application clears a field and then writes its new
/// value starting from the same position.
#[test]
fn erasing_characters_leaves_the_cursor_on_its_column() {
    let mut screen = screen();
    for c in ['a', 'b', 'c'] {
        screen.print(c);
    }
    screen.state.column = GridColumn(1);
    screen.erase_chars(2);
    assert_eq!(screen.state.column, GridColumn(1));
    assert_eq!(screen.state.line, ScreenLine(0));
}
```

## TC-06 — 消去セルは属性が落ち、背景は pen のものになる

| | |
| - | - |
| Setup | `screen()`; pen を 前景 `Indexed(1)` / 背景 `Indexed(2)` / `Style::BOLD` にして `'a','b','c'` を `print`; その後 pen を 前景既定 / 背景 `Indexed(4)` / 装飾なし に変える; カーソルを `GridColumn(1)` |
| Act | `screen.erase_chars(1)` |
| Expect | 列1の `.c` が `' '` **[C11]** ／ 列1の `.fg` が `Color::DefaultForeground` **[C11]** ／ 列1の `.style` が空 **[C11]** ／ 列1の `.bg` が `Color::Indexed(4)`（pen の背景） **[BCE]** ／ 列0は `.c == 'a'` と `.fg == Color::Indexed(1)` と `Style::BOLD` のまま **[C12]** |

**これが無いと、空白を書くだけで属性を触らない実装が TC-01〜TC-05 を全て通過する。**
既存の消去テストは `.c` しか見ていないものが多く（`erase_in_line.rs` の1件だけが `.bg` を
見ている）、ECH でも同じ穴が空くところだった。

前後で pen を変えるのは、消去セルの背景が「消去**時点**の pen」由来であって「文字を書いた
ときの pen」由来ではないことを分けるため。両者が同じ色だと、どちらを読んでいても通ってしまう。
列0を検証するのは C12 の「他の文字に影響しない」が属性にも及ぶことを見るため。

```rust
/// Asserts that erased cells lose the foreground and styling they
/// carried and take the pen's background.
///
/// Case: an application clears a field that was drawn in bold on a
/// colored background, while its pen now carries a different one.
#[test]
fn erasing_characters_clears_the_attributes_and_takes_the_pen_background() {
    let mut screen = screen();
    screen.pen_mut().fg = Color::Indexed(1);
    screen.pen_mut().bg = Color::Indexed(2);
    screen.pen_mut().style = Style::BOLD;
    for c in ['a', 'b', 'c'] {
        screen.print(c);
    }
    screen.pen_mut().fg = Color::DefaultForeground;
    screen.pen_mut().bg = Color::Indexed(4);
    screen.pen_mut().style = Style::empty();
    screen.state.column = GridColumn(1);
    screen.erase_chars(1);
    assert_eq!(screen.grid[ScreenLine(0)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(0)][1].fg, Color::DefaultForeground);
    assert_eq!(screen.grid[ScreenLine(0)][1].style, Style::empty());
    assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(4));
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid[ScreenLine(0)][0].fg, Color::Indexed(1));
    assert_eq!(screen.grid[ScreenLine(0)][0].style, Style::BOLD);
}
```

## TC-07 — `CSI Pn X` がディスパッチを通って消去し、カーソルを動かさない

| | |
| - | - |
| Setup | `interpret(b"abcd\x1b[1;2H\x1b[2X")`（4x3 のグリッド） |
| Act | 上記チャンクの解釈そのもの |
| Expect | 列0は `'a'` **[C5]** ／ 列1・列2が空白 **[C1][C5]** ／ 列3は `'d'` **[C1]** ／ `cursor_column()` が `GridColumn(1)` **[C10]** |

**この一覧で唯一、実際に起きたバグを検出できるケース。** `csi_dispatch` に `b'X'` の分岐が
無ければ `CSI 2 X` は `_ => {}` に落ち、`abcd` がそのまま残ってここで落ちる。TC-01〜TC-06 は
`Screen::erase_chars` を直接呼ぶので、分岐の有無を一切見ていない。

`\x1b[1;2H` は 1-based の CUP なので 0-based の列1にカーソルが座る。`abcd` を4桁に書くと
deferred wrap が張られるが、続く CUP がカーソルを動かすので ECH には影響しない。

```rust
/// Asserts that `CSI Pn X` reaches the screen and erases `Pn` cells
/// from the cursor without moving it.
///
/// Case: Neovim clears the padding of a file-tree pane, which does
/// not reach the right edge of the screen.
#[test]
fn the_erase_character_sequence_clears_the_span_at_the_cursor() {
    let device = interpret(b"abcd\x1b[1;2H\x1b[2X");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[2].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[3].c, 'd');
    assert_eq!(screen.cursor_column(), GridColumn(1));
}
```

## TC-08 — 省略・0・1 のいずれも1セルだけ消す

| | |
| - | - |
| Setup | `interpret(b"abc\x1b[1;1H\x1b[X")` / `\x1b[0X` / `\x1b[1X` の3通り |
| Act | 各チャンクの解釈 |
| Expect | いずれも列0が空白 **[C4][C9]** ／ 列1は `'b'` のまま **[C4]** |

`Pn` の既定値と 0 の扱い（C4 / C9）は `repeat_count` が担うので Screen 層では観測できない。
ECH としてこの契約を固定できるのはこの層だけ。`repeat_count` 自体には直接のテストが無く、
既存の固定は `line_editing.rs` が `CSI 0 M` / `CSI T` / `CSI 0 T` で個別に行っている形なので、
ECH も同じ形で自分の分を持つ。

3通りを順に並べるのは既存の `the_scroll_down_sequence_scrolls_the_region_down` が
`CSI T` と `CSI 0 T` を1テスト内で続けて確かめている書き方に合わせたもの。

```rust
/// Asserts that an omitted, zero, or explicit one erase count all
/// erase a single cell.
///
/// Case: a program emits the terminfo `ech` capability, whose
/// parameter it leaves at the default.
#[test]
fn an_omitted_or_zero_erase_character_count_erases_one_cell() {
    let device = interpret(b"abc\x1b[1;1H\x1b[X");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, 'b');

    let device = interpret(b"abc\x1b[1;1H\x1b[0X");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, 'b');

    let device = interpret(b"abc\x1b[1;1H\x1b[1X");
    let screen = device.active_screen();
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, ' ');
    assert_eq!(screen.viewport_row(ViewportLine(0))[1].c, 'b');
}
```

---

# 付録

## 契約表

ページ番号は **PDF のページ番号**（`\f` を数えたもの）。紙面の印刷ページとは異なる。

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C1 | ECH | unconditional | This control function erases one or more characters, from the cursor position to the right. | vt510.pdf p.309, L9028 |
| C2 | ECH | unconditional | ECH clears character attributes from erased character positions. | vt510.pdf p.309, L9028-9029 |
| C3 | ECH | unconditional | ECH works inside or outside the scrolling margins. | vt510.pdf p.309, L9029-9030 |
| C4 | ECH | numeric parameter | is the number of characters to erase. A Pn value of 0 or 1 erases one character. Default: Pn = 1. | vt510.pdf p.309, L9039-9040 |
| C5 | ECH | unconditional | Erases characters at the cursor position and the next Pn-1 characters. | vt220.pdf p.36, L1750 |
| C6 | ECH | unconditional | Character attributes are set to normal. No reformatting of data on the line occurs. | vt220.pdf p.36, L1751-1752 |
| C6b | ECH | unconditional | The cursor remains in the same position. | vt220.pdf p.36, L1752-1753 — **REJECTED**、C10 が代替 |
| C7 | ECH / ERM | mode-dependent | Whether the character positions of protected areas are put into the erased state, or the character positions of unprotected areas only, depends on the setting of the ERASURE MODE (ERM). | ECMA-48.pdf p.55（印刷 p.41）, L2482-2483 |
| C8 | ECH | mode-dependent | Available in: VT Level 4 mode only | vt510.pdf p.309, L9031 |
| C9 | ECH | numeric parameter | Parameter default value: Pn = 1 | ECMA-48.pdf p.55（印刷 p.41）, L2475 |
| C10 | 消去全般 | unconditional | The cursor position does not change when erasing characters or lines. | vt220.pdf p.36, L1744 |
| C11 | 消去全般 | unconditional | Erasing a character also erases any character attribute of the character. | vt220.pdf p.36, L1744-1745 |
| C12 | 消去全般 | unconditional | Erasing removes characters from the screen without affecting other characters on the screen. | vt220.pdf p.36, L1743 |

C10 / C11 / C12 は vt220 §4.12「Erasing」の導入部の平文で、表組みではないため機械検証を
そのまま通る。ECH 固有ではなく消去操作全般の規定だが、ECH はその節が列挙する消去操作の
一つとして直後の表に載っている。

kind-2 レジャー:

```
BCE — crates/orzma_vt/src/screen/cell.rs:84-85, Pen::erase_cell
      "Builds the blank cell erase operations write: the pen's
       background with default foreground and no styling (BCE)."
      justifies: 消去セルは pen の背景を保ち、前景は既定、装飾なし

LC  — crates/orzma_vt/src/screen.rs:4-7, ファイルの //! ヘッダ
      "a mutation that damages rows returns the [`DamageSpan`] it
       produced for the caller to stage, and pure cursor motion
       returns nothing, because the per-chunk cursor diff reports it"
      justifies: 行を変えた変更は DamageSpan を返し、カーソル移動だけなら返さない
```

**citations verified: 14/15**（マニュアル 12/13、kind-2 2/2。棄却は C6b のみ）

## 探索した語

| Level | 出所 | 語 | 結果 |
| - | - | - | - |
| 1 | メソッド自身の `///` | — | `NOTFOUND`（メソッド未存在。新規実装のため想定内） |
| 2 | 囲む impl の doc `/// Erasure.` | "erase", "erasure" | vt220 §4.12「Erasing」に到達（C10-C12 の出所） |
| 3 | `screen.rs` の `//!` ヘッダ | DamageSpan 返却契約 | kind-2 `LC` を採取 |
| 4 | メソッド名 + 型名 | "ECH", "Erase Character", "CSI Pn X" | vt510 / vt220 / ECMA-48 の3冊すべてで定義に到達 |

到達できなかった語は無い。マニュアル別の内訳:

- **vt510**: `ECH` は L344（目次）・L3153（一覧表）・L6640（相互参照）を棄却し、L9027 の
  定義節に到達。棄却 3 件。
- **vt220**: `ECH` / `Erase Character` で L1750 の表に到達。さらに `erasing` で §4.12 の
  導入部 L1743-1745 に到達。棄却 0 件。
- **ECMA-48**: `ECH` は L298（目次）・L961・L1545・L1553・L1799・L1957（相互参照）を棄却し、
  L2472 の 8.3.38 に到達。棄却 6 件。

precedence の降下は行っていない。vt510 が C1〜C4 / C8 を、vt220 が vt510 の沈黙する
C5 / C6 / C10 / C11 / C12 を、ECMA-48 が両者の沈黙する C7 / C9 を埋めており、
**3冊が矛盾する箇所は無い**。

## 仕様にないもの

| 項目 | 内容 |
| - | - |
| **C6b（棄却された citation）** | 引用 "The cursor remains in the same position."、span vt220.pdf L1752-1753。結果 **REJECTED (dropped qualifier: only)**。原因は組版で、左端の Name 列 `(VT200 mode only)` が L1752→L1753 に折り返し、`only)` が "The cursor remains" と "in the same position" の**間**に割り込む。span を狭めても文が2行にまたがるため除去できない。**同じ内容を平文で述べる C10 が見つかったので、TC-05 はそちらを典拠にしている**。C6b は経緯として残す |
| **行末クリップ（TC-02）** | どのマニュアルも、行に残っている数より多く要求されたときの振る舞いを規定していない。TC-02 が固定するのは xterm 互換の契約であり、仕様からの導出ではない |
| **pending wrap との相互作用** | vt510 / vt220 / ECMA-48 のいずれも ECH と deferred wrap の関係を述べていない。**当初この一覧は固定を見送ったが、コードレビューで EL との不整合が指摘され、実測を根拠に決着させて TC-09 を追加した**（下記「決着: deferred wrap」）。マニュアル由来ではないので Source は `WRAP` |
| **C7（ERM / 保護領域）** | O blocker。ECMA-48 は保護領域の消去可否が ERASURE MODE に依存すると述べるが、orzma は DECSCA も ERM もモデル化していない。`Setup:` がその状態を作れず、`Expect:` が観測もできない |
| **C8（VT Level 4 mode only）** | O blocker。適合レベルをモデル化していないため、条件を成立させられない |

## 決着: deferred wrap（TC-09 の根拠）

マニュアルは **EL 0 がカーソル位置を含む**と明言している。

- **C13** vt220 PDF p.36 L1754: "Erases from the cursor to the end of the line,
  including the cursor position." → **VERIFIED**
- **C14** vt510 PDF p.311 L9074: "From the cursor through the end of the line" → **VERIFIED**

しかし**どのマニュアルも deferred wrap をモデル化していない**ため、wrap が張られた状態で
カーソルが論理的にどこに居るかを裁定しない。ここが唯一の争点で、2つの自己整合なモデルがある。

| モデル | カーソル位置 | wrap 中の消去 |
| - | - | - |
| A | 最終列の1つ先 | 右側に何も無いので消さない |
| B | 最終列 + フラグ | 最終列を消す |

**実測（tmux 3.7c、幅10の行を `0123456789` で埋めた状態）:**

| 操作 | 結果 |
| - | - |
| 行中で `CSI 2 X` | `01..456789` — 正常に動作 |
| 行中で `CSI 0 K` | `01........` — 正常に動作 |
| wrap 中に `CSI 1 X` | `0123456789` — **何も消えない** |
| wrap 中に `CSI 0 K` | `0123456789` — **何も消えない** |
| wrap 中に `CSI 1 X` の後 `Z` | `Z` は次行へ — **wrap は保持** |

tmux はモデル A（`screen_write_clearcharacter` が `cx > sx - 1` で早期 return）。

> **訂正。** 当初この節は「alacritty も同じ」と書いていたが、**これは誤りだった**。
> ソースを確認すると alacritty は EL と ECH を**意図的に区別している**:
> `alacritty_terminal-0.26.0/src/term/mod.rs:1643` の `clear_line` は
> `LineClearMode::Right if cursor.input_needs_wrap => return` を持つ一方、
> 同 1519-1535 の `erase_chars` には `input_needs_wrap` の判定が**一切無く**、
> wrap 中でも最終列を消す。xterm の `CASE_ECH` も `do_wrap` を見ない（未実測）。
> **ECH の no-op を支持する参照実装は tmux 1つだけ**である。

参照実装が割れていることを**承知した上で A を採用**し、`erase_chars` に `EL 0` と同じ
no-op ガードを置いた。`erase_in_line` は無変更。Source タグ `WRAP` はマニュアル由来では
なく、この選択そのものを指す。実 nvim のキャプチャでは ECH は全て行中発行でこの境界を
踏まないため、今回のバグ修正の妥当性には影響しない。詳細と再訪の条件は
`vt-conformance-scope.md` §4。

## 仕様の解釈（旧「仕様の矛盾」）

**消去セルの背景** — 当初は仕様矛盾としてケース化を見送ったが、レビューを経て
**TC-06 として採用**した。

- 仕様側 **C2 / C6 / C11**: 文字属性は normal に戻る／消去は文字属性も消す
- repo 側 **BCE**（`cell.rs:84-85`）: pen の背景を保ち、前景は既定、装飾なし

前景と装飾は一致する。食い違うのは背景だけで、そこは **DEC のマニュアルが色を文字属性に
含める以前の記述**であり、`bce` capability がまさに宣言している逸脱。orzma は EL / ED で
既に BCE で確定しており、`xterm-256color` は `bce` を広告している。**未解決の矛盾ではなく、
選択した xterm 互換契約として記録する** — したがって TC-06 の背景の行だけ [BCE] タグ、
前景と装飾の行は [C11] タグになる。

## API 改定

新規メソッドのため既存 signature の改定は無い。合意した形:

```rust
pub fn erase_chars(&mut self, count: u16) -> Option<DamageSpan>
```

著者が選択した層分け（`count` は正規化済み、`repeat_count` は interpreter 側）の帰結として、
C4 / C9 は Screen 層ではケース化されず、**TC-08 が interpreter 層で固定する**。

## 出典なしの改善提案

仕様に根拠を持たない、著者への申し送り。ケースには影響しない。

- **`repeat_count` に直接のテストが無い。** `interpreter.rs` の **12 箇所**（CUU / CUD /
  CUF / CUB / CNL / CPL / IL / DL / SU / SD / CHT / CBT）から使われ、ECH で 13 箇所目になる。
  ヘルパ自体を直接叩くテストは無いが、`line_editing.rs` が `CSI 0 M` / `CSI T` / `CSI 0 T` で
  制御機能ごとに 0 と既定を押さえている形。ECH の分は TC-08 が持つので追加の起票は不要だが、
  ヘルパ直接のテスト（`None` / `0` / `1` / 大きい値）を足すのは安価な補強。
- **EL の pending-wrap 例外は ECH と隣接する。** `screen.rs:574` は deferred wrap 中の
  `EL 0` を何もせず返す。ECH 側には同じ例外を置かない（典拠が無い）ので、実装後は最終桁で
  `CSI 1 X` は消えて `CSI K` は消えない、という食い違いが観測可能になる。ECH 実装を
  阻害はしないが、`vt-conformance-scope.md` §4 の判断が未了である点は残る。

## docs/todo との衝突

- **`nvim-tree-stale-cells-ech.md` §6.1** が同じ `erase_chars` の実装を既に載せており、
  signature は本書の合意と**一致**する（`count: u16` / `Option<DamageSpan>`）。あちらは実装、
  本書はテストで、役割が分かれている。矛盾ではないが、片方を変えるならもう片方も直すこと。
- **`vt-conformance-scope.md` §4 / §5-1** は「ECH 実装と同時に EL の pending-wrap 例外を
  再判断する」を推奨している。本書のケース一覧は**その再判断を含まない**（EL は別メソッドで、
  ECH の pending-wrap 挙動には典拠が無い）。この不一致は解消していない — どちらを採るかは
  著者の判断。
