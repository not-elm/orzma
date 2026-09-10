# Test cases: Screen::print

作成日: 2026-09-11 / 対象リビジョン: `4778a25`

`Screen::print` が **IRM（Insert/Replace Mode, `CSI 4 h` / `CSI 4 l`）** に対応する
ぶんのテストケース。参照したのは `docs/references/` の `vt510.pdf`（IRM p.319、
ICH p.316、DECAWM p.129）と `ECMA-48.pdf`（7.1 モードの概念、7.2.9 HEM、7.2.10 IRM、
7.3.3 HEM×IRM、8.3.117 SGR）。**citations verified: 15/15**（マニュアル 12 件、
リポジトリの doc コメント 3 件）。

Phase 4 の結果は「ケース一覧8件を承認、signature 提案は取らず現状維持」。
取らなかった提案は付録「API 改定」に citation 付きで残してある。

> **2026-09-11 の見直しで TC-06 を差し替えた。** xterm・alacritty・kitty・ghostty・
> foot・wezterm の実装を確認した結果、TC-06 の2つの主張自体は6実装すべてと一致して
> いたが、初版のテストはそのどちらも実際には識別できていなかった（属性が一様だった
> ため）。同じ見直しで、付録「仕様にないもの」にあった「行に限られるかはマニュアルが
> 決めていない」という記述が誤りだと判明し、C10 として契約表に繰り上げた。詳細は
> 付録「見直しの記録」。

> **このドキュメントの Rust は現状の木に対してコンパイルできない。**
> `InsertReplaceMode` はまだ存在せず、`Screen::print` は今日の木では引数を1つしか
> 取らない。下の設計を先に入れてから転記すること。
>
> ```rust
> pub fn print(&mut self, c: char, mode: InsertReplaceMode) -> Option<DamageSpan>
> ```
>
> **どのテストもビルドしていない。** この skill はコンパイルを走らせないし、
> 合意した signature がまだ木に無い以上、走らせようもない。貼って動くものでは
> なく、転記する提案として読むこと。

このリストは仕様から導いて自己検証しただけで、他の誰もレビューしていない。
信用する前に引用を1〜2件だけ実際の PDF に当てて確かめてほしい。

## テストケース一覧

優先度順（High → Medium）に並べてある。ID は Phase 4 で提示したものを
そのまま使っているので、番号は優先度順には並んでいない。High まで読めば、
仕様が明言している振る舞いは全部揃う。

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `an_insert_mode_print_shifts_the_row_right_and_drops_the_last_cell` | C1, C2, C4, C5, C6, C7, Damage | High |
| TC-02 | `a_replace_mode_print_overwrites_the_cell_without_shifting_the_row` | C3, Damage | High |
| TC-03 | `an_insert_mode_print_at_the_last_column_replaces_the_cell_it_pushes_out` | C1, C2, C4, Damage | High |
| TC-08 | `an_insert_mode_print_into_a_padded_row_loses_no_visible_cell` | C1, C2, C4, C6, Damage | High |
| TC-04 | `an_armed_deferred_wrap_resolves_before_the_insert_shifts_the_new_row` | C9, C1, C4, C6, C10, DW, Damage | Medium |
| TC-06 | `an_insert_mode_print_keeps_the_shifted_cells_attributes` | C4, C7, C12, IC | Medium |
| TC-07 | `an_insert_mode_print_on_a_single_column_screen_replaces_the_only_cell` | C2, C4, Damage | Medium |

Source タグ — **C1**: vt510 p.319 L9185-9186（set で右へずれる）／**C2**: vt510
p.319 L9185-9186（右端を越えた文字は失われる）／**C3**: vt510 p.319 L9187（reset
で上書き）／**C4**: vt510 p.319 L9174-9175（常にカーソル位置に追加）／**C5**:
ECMA-48 p.38 L1633-1635（INSERT の定義）／**C6**: ECMA-48 p.41 L1785-1788（IRM=INSERT
＋HEM=FOLLOWING でカーソルは通常どおり進む）／**C7**: ECMA-48 p.37 L1600-1602（挿入は
アクティブ位置とそれ**以降**を character path 方向へずらす）／**C8**: ECMA-48 p.34
L1449-1450（reset 状態が定義の先頭）／**C9**: vt510 p.129 L4592-4594（DECAWM set の
折り返し）／**C10**: vt510 p.316 L9148-9149（シフトはカーソルと右マージンの間、行内に
限られる）／**C12**: ECMA-48 p.75 L3474-3476（SGR で確立した rendition が後続テキストに
効く）／**DW**・**IC**・**DMG**: リポジトリの doc コメント（付録の kind-2 台帳）／
**Damage**: `Option<DamageSpan>` の判定表。全エントリの引用文は付録の契約表にある。

8件のうち仕様由来でないものは無い（`TC-A` 番号のケースは出ていない）。kind-2 タグが
残るのは TC-06 の「動いたセルが自分の属性を保つ」1行だけで、そこは **IC** が根拠に
なる。マニュアルはシフトが行内に限られること（C10）までは述べるが、動いたセルの
属性については沈黙している（付録「仕様にないもの」を参照）。

テストコードは `crates/orzma_vt/src/screen/tests/print.rs`（`mod tests::print`）へ
追加する。既存の `screen()`・`seed_row`・`row_glyphs` ヘルパをそのまま使う。
TC-06 だけ `use crate::screen::grid::run::Style;` が要る（`insert_characters.rs`
と同じ形）。`InsertReplaceMode` は `screen.rs` が signature のために import する
ので、`tests.rs` → `tests/print.rs` の `use super::*` 2段で届く。

---

## TC-01 — insert mode は行を右へずらし、文字はカーソル位置に載る 〈High〉

| | |
| - | - |
| Setup | `screen()`（4桁×3行）; 行0を `['a','b','c','d']` で埋める; カーソルを `GridColumn(1)` へ |
| Act | `screen.print('X', InsertReplaceMode::Insert)` |
| Expect | 行0が `['a','X','b','c']` になる **[C1][C4][C5][C7]** ／ 右端へ押し出された `'d'` は失われる **[C2]** ／ カーソルは `GridColumn(2)` **[C6]** ／ `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

行を満杯にしておくのが要点で、C1（右シフト）と C2（右端の脱落）を1つの act で
同時に踏む。`'b'` が桁2に来ることが「カーソルセル自身も動く」＝C7 の *the active
presentation position and … the following character positions* を pin し、`'X'` が
桁1に載ることが C4 の「常にカーソル位置に追加する」を pin する。ずらしてから
カーソルを進めて桁2に書いてしまう実装は、この2行が同時に外れて落ちる。

```rust
/// Asserts that a print in insert mode shifts the cells at and right of
/// the cursor one column right, stamps the character at the cursor, and
/// drops the cell pushed past the last column.
///
/// Case: a line editor is in insert mode and the user types a character
/// into the middle of a command that already fills the row.
#[test]
fn an_insert_mode_print_shifts_the_row_right_and_drops_the_last_cell() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'X', 'b', 'c']);
    assert_eq!(screen.state.column, GridColumn(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-02 — replace mode は行をずらさない 〈High〉

| | |
| - | - |
| Setup | `screen()`; 行0を `['a','b','c','d']` で埋める; カーソルを `GridColumn(1)` へ |
| Act | `screen.print('X', InsertReplaceMode::Replace)` |
| Expect | 行0が `['a','X','c','d']` になる **[C3]** ／ カーソルは `GridColumn(2)`（*仕様由来ではない* — 下記）／ `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

新しい引数が入ったあとに「常にずらす」実装へ滑るのを防ぐ側の対。

**カーソル前進の行には tag が無い。** これは 2026-09-11 のレビューで、insert 側の
双子（TC-01）が持つ assertion が replace 側だけ欠けていて、replace 分岐でカーソルが
2桁進む（あるいは進まない）退行を誰も捕まえられない、と指摘されて足したもの。
replace mode のカーソル前進を述べたマニュアルの文は契約表に無い — **C6**（ECMA-48
§7.3.3b）は `IRM` が **INSERT** のときの記述で、replace 側には及ばない。既存の
`print_stamps_the_pen_and_advances` が持つ通常の print の契約を、この分岐でも
確かめる退行テストとして置いている。仕様由来の期待ではないので、そう明記する。

```rust
/// Asserts that a print in replace mode overwrites the cell at the
/// cursor and leaves the rest of the row where it was.
///
/// Case: an ordinary shell, which never set IRM, echoes a character over
/// text already on the row.
#[test]
fn a_replace_mode_print_overwrites_the_cell_without_shifting_the_row() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(1);
    let damage = screen.print('X', InsertReplaceMode::Replace);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'X', 'c', 'd']);
    assert_eq!(screen.state.column, GridColumn(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-03 — 最終桁での insert はずらす先が無く、replace と同じ結果になる 〈High〉

| | |
| - | - |
| Setup | `screen()`; 行0を `['a','b','c','d']` で埋める; `screen.state.column = GridColumn(3)`（deferred wrap は立てない） |
| Act | `screen.print('X', InsertReplaceMode::Insert)` |
| Expect | 行0が `['a','b','c','X']` になる **[C1][C2][C4]** ／ カーソルは `GridColumn(3)` のまま ／ deferred wrap が立つ ／ `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

C2 の条件が満たされる側の境界。右へ動かされる文字が `'d'` ただ1つで、それが
右端を越えて失われるため、シフトは観測不能になり書き込みだけが残る。シフト量を
`clamped_columns` に通さず 0 に丸めて early return する実装だと `'d'` が桁3に
残るので、ここで落ちる。deferred wrap の期待は、insert 分岐が通常の print の
後始末を飛ばしていないことを併せて見るために置いてある。

```rust
/// Asserts that a print in insert mode at the last column drops the cell
/// it pushes past the border and stamps the character there, arming the
/// deferred wrap as an ordinary print would.
///
/// Case: a program in insert mode types into the rightmost column of a
/// full row.
#[test]
fn an_insert_mode_print_at_the_last_column_replaces_the_cell_it_pushes_out() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'c', 'd']);
    screen.state.column = GridColumn(3);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'X']);
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(screen.state.pending_wrap);
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-08 — 行に余白があるとき insert は可視の内容を落とさない 〈High〉

| | |
| - | - |
| Setup | `screen()`; 行0を `['a','b']` で埋める（桁2・桁3は既定セルのまま）; カーソルを `GridColumn(1)` へ |
| Act | `screen.print('X', InsertReplaceMode::Insert)` |
| Expect | 行0が `['a','X','b',' ']` になる **[C1][C4]** ／ 桁3は空白のまま **[C2]** ／ カーソルは `GridColumn(2)` **[C6]** ／ `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

C2 の条件が**満たされない**側で、TC-01 の対。右端を越えるのが既定セルなので
何も失われない。桁3に来るのは押し出された既定セルで、桁1に開くのは pen の
erase セル — どちらも空白なので `' '` 1つで見分けなく比較できる。行が満杯の
場合しか試さないと、シフト量を行幅から数える実装（余白まで巻き込んで詰める）を
見逃す。

```rust
/// Asserts that a print in insert mode into a row with trailing blanks
/// shifts the text right without losing any visible cell.
///
/// Case: a program in insert mode types into a half-filled row.
#[test]
fn an_insert_mode_print_into_a_padded_row_loses_no_visible_cell() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b']);
    screen.state.column = GridColumn(1);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'X', 'b', ' ']);
    assert_eq!(screen.state.column, GridColumn(2));
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

## TC-04 — deferred wrap は insert より先に解決され、シフトは折り返し先の行に効く 〈Medium〉

| | |
| - | - |
| Setup | `screen()`; 行0に `'a'`〜`'d'` を Replace で印字して deferred wrap を立てる; 行1を `['p','q','r','s']` で埋める |
| Act | `screen.print('X', InsertReplaceMode::Insert)` |
| Expect | 行1が `['X','p','q','r']` になる **[C9][C1][C4][DW]** ／ 行0は `['a','b','c','d']` のまま **[C10]** ／ カーソルは `ScreenLine(1)`, `GridColumn(1)` **[C6]** ／ `Some(DamageSpan::rows(ViewportLine(1), ViewportLine(1)))` **[Damage]** |

優先度は Medium（DECAWM 常時オンという orzma 側の選択に依存する）だが、
**実装がぶら下がっているのはこのケース**。シフトを wrap 解決より前に置くと
`insert_characters` が `pending_wrap` を落とし、`'X'` は行0の最終桁を上書きして
行1は無傷のまま — 期待の4行が同時に外れる。行1を事前に埋めておくのは、シフトが
実際に行1へ効いたことを観測可能にするため（空行のままだとシフトの有無が見えない）。
行0を据え置きで確認するのは、シフトが折り返し元の行に漏れていないことを見る
ためで、根拠は C10 の *Text between the cursor and right margin moves to the right*
— "page's right border" ではなく右マージンまで、つまり行内。

```rust
/// Asserts that an armed deferred wrap is resolved before the insert
/// shift, so the shift lands on the row the wrap moved to and the row it
/// left is untouched.
///
/// Case: a program in insert mode fills a row exactly and then types one
/// more character, which continues on the next line.
#[test]
fn an_armed_deferred_wrap_resolves_before_the_insert_shifts_the_new_row() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace);
    }
    seed_row(&mut screen, ScreenLine(1), &['p', 'q', 'r', 's']);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['a', 'b', 'c', 'd']);
    assert_eq!(row_glyphs(&screen, ScreenLine(1)), vec!['X', 'p', 'q', 'r']);
    assert_eq!(
        (screen.state.line, screen.state.column),
        (ScreenLine(1), GridColumn(1))
    );
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(1), ViewportLine(1)))
    );
}
```

## TC-06 — 動いたセルは自分の属性を保ち、挿入された文字は現在の pen を持つ 〈Medium〉

| | |
| - | - |
| Setup | `screen()`; `'a'`〜`'d'` を **1桁ずつ違う背景色**（`Indexed(1)`,`(2)`,`(3)`,`(5)`）で Replace 印字; pen を `Style::ITALIC` + `fg = Indexed(6)` + `bg = Indexed(4)` に変更; カーソルを `GridColumn(1)` へ戻し、deferred wrap を落とす |
| Act | `screen.print('X', InsertReplaceMode::Insert)` |
| Expect | 桁1が `'X'`・`Style::ITALIC`・`fg = Indexed(6)`・`bg = Indexed(4)` **[C4][C12]** ／ 桁2が `'b'` で `bg = Indexed(2)`、桁3が `'c'` で `bg = Indexed(3)` **[IC][C7]** |

**シフト元のセルに互いに違う背景を与えるのが要点。** 桁2に来るのは `'b'` で、
その背景は **`'b'` 自身の `Indexed(2)`** でなければならない。桁2がもともと持って
いた `Indexed(3)` が残っていたら、それは文字だけ動かして属性を据え置いた実装。
行全体を同じ属性で塗ると、この2つが同じ値になって区別が付かなくなる。

**挿入セル側で `fg` と `style` まで見るのも同じ理由。** `Pen::erase_cell` は
「pen の背景・既定の前景・装飾なし」を返すので、pen の `fg` が既定で `style` が空
だと `erase_cell` と `stamp` の結果が glyph 以外まったく同じになり、「開いた桁の
erase セルの `.c` を差し替えるだけ」の実装まで通ってしまう。pen に非既定の `fg` と
非空の `style` を持たせて初めて `stamp` を通ったことが確かめられる。

**setup に落とし穴が1つある。** 4文字を印字した直後は deferred wrap が立って
いるので、カーソル桁を代入し直すだけでは足りない — 明示的に落とさないと act の
print が折り返してしまう。`insert_characters` の同型テストはこれを省いても通る
（`insert_characters` 自身が `pending_wrap` を落とすため）ので、そちらから
写すときに漏れやすい。damage の期待は置いていない。TC-01 が同じ形で既に pin
しており、このケースが見たいのは属性だけだから。

**このテストは開いた桁の埋め方を pin しない。** 直後の glyph が同じ桁を完全に
覆うので、erase セルで埋めてから上書きする実装でも、まったく埋めない実装でも
同じように通る（実際 alacritty・foot・wezterm の IRM 経路は埋めない）。埋めの形
そのものは、観測できる場所 — `insert_characters` 上の
`an_inserted_blank_carries_the_pen_background_without_its_rendition` — が既に
pin している。

```rust
/// Asserts that an insert-mode print stamps the whole current pen at the
/// cursor while each cell the shift moves keeps the attributes it was
/// printed with.
///
/// Case: a TUI in insert mode types a character into a row it had drawn
/// one cell at a time in different colors.
#[test]
fn an_insert_mode_print_keeps_the_shifted_cells_attributes() {
    let mut screen = screen();
    for (c, bg) in [('a', 1), ('b', 2), ('c', 3), ('d', 5)] {
        screen.pen_mut().bg = Color::Indexed(bg);
        screen.print(c, InsertReplaceMode::Replace);
    }
    screen.pen_mut().style = Style::ITALIC;
    screen.pen_mut().fg = Color::Indexed(6);
    screen.pen_mut().bg = Color::Indexed(4);
    screen.state.column = GridColumn(1);
    screen.state.pending_wrap = false;
    screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(screen.grid[ScreenLine(0)][1].c, 'X');
    assert_eq!(screen.grid[ScreenLine(0)][1].style, Style::ITALIC);
    assert_eq!(screen.grid[ScreenLine(0)][1].fg, Color::Indexed(6));
    assert_eq!(screen.grid[ScreenLine(0)][1].bg, Color::Indexed(4));
    assert_eq!(screen.grid[ScreenLine(0)][2].c, 'b');
    assert_eq!(screen.grid[ScreenLine(0)][2].bg, Color::Indexed(2));
    assert_eq!(screen.grid[ScreenLine(0)][3].c, 'c');
    assert_eq!(screen.grid[ScreenLine(0)][3].bg, Color::Indexed(3));
}
```

## TC-07 — 1桁の画面では insert が唯一のセルを置き換える 〈Medium〉

| | |
| - | - |
| Setup | `Screen::new(GridSize { cols: 1, rows: 1 }, 10)`; 行0を `['a']` にする; カーソルは `GridColumn(0)` の初期位置のまま |
| Act | `screen.print('X', InsertReplaceMode::Insert)` |
| Expect | 行0が `['X']` になる **[C2][C4]** ／ deferred wrap が立つ ／ `Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))` **[Damage]** |

TC-03 と同じ C2 の境界を、行幅そのものが 1 の極値で踏む。`copy_within` に
渡すレンジが `0..0` へ潰れる唯一の形なので、シフトのレンジ計算が幅1で破綻
しないことを併せて見る。`Screen` の不変条件は両軸が非零であることだけなので、
1桁は正当な入力。

```rust
/// Asserts that an insert-mode print on a one-column screen replaces the
/// only cell, because the character it shifts falls past the border.
///
/// Case: the window is dragged down to a single column while a program
/// in insert mode keeps printing.
#[test]
fn an_insert_mode_print_on_a_single_column_screen_replaces_the_only_cell() {
    let mut screen = Screen::new(GridSize { cols: 1, rows: 1 }, 10);
    seed_row(&mut screen, ScreenLine(0), &['a']);
    let damage = screen.print('X', InsertReplaceMode::Insert);
    assert_eq!(row_glyphs(&screen, ScreenLine(0)), vec!['X']);
    assert!(screen.state.pending_wrap);
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(0)))
    );
}
```

---

# 付録

## 契約表

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C1 | IRM | mode-dependent | "If IRM mode is set, then new characters move characters in page memory to the right." | vt510.pdf p.319, L9185-9186 |
| C2 | IRM | conditional | "Characters moved past the page's right border are lost." | vt510.pdf p.319, L9185-9186 |
| C3 | IRM | mode-dependent | "If IRM mode is reset, then new characters replace the character at the cursor position." | vt510.pdf p.319, L9187 |
| C4 | IRM | unconditional | "This control function selects how the terminal adds characters to page memory. The terminal always adds new characters at the cursor position." | vt510.pdf p.319, L9174-9175 |
| C5 | IRM (ECMA-48 7.2.10) | mode-dependent | "The graphic symbol of a graphic character or of a control function, for which a graphical representation is required, is inserted at the active presentation position." | ECMA-48.pdf p.38, L1633-1635 |
| C6 | IRM×HEM (ECMA-48 7.3.3 b) | mode-dependent | "If the IRM is set to INSERT, then, in addition, the effect of the receipt of a graphic character or a control function for which a graphical representation is required, depends on the setting of the HEM. If the HEM is set to FOLLOWING, the implicit movement of the active position is performed normally" | ECMA-48.pdf p.41, L1785-1788 |
| C7 | HEM (ECMA-48 7.2.9) | mode-dependent | "a character insertion causes the contents of the active presentation position and of the following character positions in the presentation component to be shifted in the direction of the character path" | ECMA-48.pdf p.37, L1600-1602 |
| C8 | Modes (ECMA-48 7.1) | unconditional | "The reset state is shown first in the definitions in 7.2." | ECMA-48.pdf p.34, L1449-1450 |
| C9 | DECAWM | mode-dependent | "If the DECAWM function is set, then graphic characters received when the cursor is at the right border of the page appear at the beginning of the next line." | vt510.pdf p.129, L4592-4594 |
| C10 | ICH | unconditional | "Text between the cursor and right margin moves to the right. Characters scrolled past the right margin are lost." | vt510.pdf p.316, L9148-9149 |
| C11 | ICH | unconditional | "The ICH sequence inserts Pn blank characters with the normal character attribute." | vt510.pdf p.316, L9147 |
| C12 | SGR (ECMA-48 8.3.117) | unconditional | "SGR is used to establish one or more graphic rendition aspects for subsequent text. The established aspects remain in effect until the next occurrence of SGR in the data stream" | ECMA-48.pdf p.75, L3474-3476 |

C10 と C11 は ICH のページの文で、IRM のページには対応する記述が無い。orzma は
IRM のシフトを `insert_characters`（＝ICH）で実装し、ECMA-48 §7.3.3 も IRM の挿入を
ICH と同じ文字挿入機構に結び付けているので、C10 はこのメソッドにも及ぶ。**C11 は
ケースを生まない** — IRM の経路では開いた桁が直後に上書きされて観測できないため。
付録「orzma と VT510 のずれ」で扱う。

行番号は `pdftotext -layout` での抽出に対する session ローカルな目印で、poppler の
版をまたいだ保証は無い。頁番号のほうは改頁文字を数えて算出している。

C8 は C6 と C7 の FOLLOWING 分岐を orzma に適用する根拠。orzma は HEM を実装して
いないので reset 状態にあり、ECMA-48 は 7.2 の定義で reset 状態を先頭に書くと
明言している。7.2.9 の先頭は FOLLOWING。

### kind-2 台帳

```
IC — crates/orzma_vt/src/screen.rs:436-438, Screen::insert_characters
     "Inserts `count` blank characters at the cursor: the cells to its
      right move right keeping their own attributes, the cells pushed
      past the last column are lost, and the cursor stays where it is."
     justifies: シフトがカーソル行に限られること／動いたセルが自分の属性を保つこと

DW — crates/orzma_vt/src/screen.rs:138-139, Screen::print
     "Prints one character at the cursor with the current pen, wrapping
      first when the deferred wrap is armed."
     justifies: 折り返しがこのメソッドの他の何よりも先に解決されること

DMG — crates/orzma_vt/src/screen.rs:144-148, Screen::print
     "A wrap that scrolled reports [`DamageSpan::Full`]; every other print
      reports the row the character landed on, or nothing when that row has
      scrolled out of the window."
     justifies: スクロールを伴う折り返しが Full を返すこと
```

## 探索した語

| Level | 語 | 到達先 |
| - | - | - |
| 1 | "wrapping"（`print` の doc 冒頭） | **DECAWM** — vt510 p.129 |
| 1 | "deferred wrap" | 到達せず（リポジトリ語彙） |
| 1 | "pen" | 到達せず（リポジトリ語彙） |
| 1 | "display width one" | 到達せず |
| 1 | `DamageSpan` | 到達せず（orzma 固有、Damage 判定表が代わり） |
| 1 | `line_feed` → LF / IND | 本メソッドの契約外として不採用 |
| 2 | "graphic character"（`impl` doc "Graphic character output."） | **ECMA-48 7.2.10**, **vt510 IRM** |
| 3 | "cursor" / "screen"（`//!` header） | 到達せず（一般語） |
| 4 | `InsertReplaceMode` → **IRM**, "Insert/Replace Mode" | vt510 p.319, ECMA-48 p.38 |
| 4 | "insert mode" / "replace mode" | 同上 |
| 4 | `char` / `print` → "active position" | **ECMA-48 7.2.9 (HEM)**, **7.3.3** |
| 4 | `Option<DamageSpan>` | 到達せず（orzma 固有） |

`print` の doc には `# Control Functions` 節が無いので、level 1 は散文から拾って
いる。vt220.pdf も IRM を持つ（4.6.4）が、vt510 が同じ振る舞いをより詳しく述べて
いるため precedence 1 で止めた。

## 仕様にないもの

引用が付かずに落としたエントリは無い。検証に落ちたエントリも無い（C5 は最初
L1633-1634 と記録して REJECTED (17/26)、実テキストに当てて L1633-1635 に記録し
直したうえで VERIFIED。落としたのではなく直したもの）。

**マニュアルが決めていないのは「動いたセルの属性」だけ。** vt510 も ECMA-48 も、
シフトが既存セルの rendition を保つのか塗り替えるのかを述べていない。TC-06 の
「桁2が `'b'` の背景を保つ」がここに寄りかかっており、根拠は kind-2 の **IC** 1本。
実装側の裏付けは強い（xterm は `MemMove(ld->attribs)` と `MemMove(ld->color)` を
並べて動かし、alacritty・kitty・ghostty・foot・wezterm も `Cell` 丸ごと動かす。
DEC STD 070 の参照アルゴリズムも `strncpy(..., sizeof(character_t))` でセル全体を
写す）が、**DEC STD 070 は `docs/references/` に無い**ので kind-1 タグにはできない。
PDF を追加すれば C タグに繰り上げられる。

**シフトが行内に限られる点は、当初「マニュアルが決めていない」と記録していたが
これは誤りだった（2026-09-11 訂正）。** IRM のページ（p.319）だけを見て "page
memory" / "page's right border" という語から判断していたが、ICH のページ（p.316）は
"Text between the cursor and **right margin** moves to the right. Characters
scrolled past the right margin are lost." と明示している。C10 として契約表に
繰り上げ、TC-04 のタグを **IC** から **C10** に付け替えた。

なお ECMA-48 8.3.115 SEE（p.74）が編集範囲の既定を `Ps = 0` = "the shifted part is
limited to the active page in the presentation component" としている点は残るが、
これは **SEE 制御機能そのもののパラメータ既定値**であって、SEE が一度も届いて
いない端末の初期編集範囲を述べていない。C10 と矛盾しない。

**HEM の PRECEDING 分岐**（ECMA-48 7.2.9, p.37）は behaviour を述べているが、
orzma は HEM を実装しないので C8 により reset 状態＝FOLLOWING に固定される。
範囲外としてケースを生まない。

**DECSTR による IRM のリセット**（vt510 Table 5-9, p.277 L8204 "Insert/replace
IRM Replace mode."）と **IRM の既定値 Replace**（vt510 p.319 L9176）は、
`Screen::print` ではなくモード型と SM/RM dispatch 側の契約なので、この run の
契約表には入れていない。`CSI 4 h` / `CSI 4 l` の dispatch は別メソッドなので、
別途 enumerate が要る。

## API 改定

**C9 の reset 分岐 — kind P blocker、現状維持で決着。**

- 引用: vt510.pdf p.129, L4595-4596 — "If the DECAWM function is reset, then graphic characters received when the cursor is at the right border of the page replace characters already on the page."
- 書けない行: `Setup:` の「DECAWM を reset にする」。`Screen::print` にも `Screen` にも DECAWM の状態を置く場所が無い。
- 提案していた signature（メソッドを越えて `AutowrapMode` の新設と DECSET 7 の配線を伴う）:

  ```rust
  pub fn print(
      &mut self,
      c: char,
      mode: InsertReplaceMode,
      autowrap: AutowrapMode,
  ) -> Option<DamageSpan>
  ```

- これで書けるようになったはずのケース: 1件（DECAWM reset で最終桁の文字が折り返さず右端を上書きする）
- 決着: **現状維持**。DECAWM は `vt-conformance-scope.md` §5 項目3 に DECSC の保存範囲の見直しとセットで別途スケジュール済みで、ここで取ると IRM の PR がその設計判断を巻き込む。C9 の **set** 分岐しか使っていない TC-04 は影響を受けない。

## 積み残し — 幅2文字のシフト量

**確認した6実装はすべて display width ぶんシフトする。** xterm は
`cells = visual_width(str, length)`（doublewide を 2 と数える）を `InsertChar` に
渡し、alacritty・ghostty は `width`、kitty は `char_width`、foot は `width`、
wezterm は `print_width` 回の `insert_cell` を回す。**幅2文字の挿入は2桁ずらす**
というのが一致した挙動。

orzma は今日そこを表現できない。`Screen::print` は「display width one」を前提に
1つの `char` を1セルに格納し、1桁だけ進める（`screen.rs:138-153` の `// TODO:`）。
提案中の `insert_characters(1)` は無条件に1桁しかずらさないので、幅2文字では
参照実装とずれる。

**これは IRM が持ち込む欠陥ではなく、既存の幅モデルの欠陥を IRM が1箇所増やす
だけ。** したがってこのバッチのケースにはしない。`screen.rs` の TODO が解けて
セル＋スペーサ表現になった時点で、IRM のシフト量も display width に合わせ、
そのときにケースを1本足す。`Run::cols` と renderer の `runs_to_cells` が既に
display width で進んでいる（`grid/run.rs:39-55`、
`orzma_tty_renderer/src/schema/grid.rs:304-345`）ので、直すときはそちらが基準。

## orzma と VT510 のずれ — ICH が開けた桁の属性

C11 が述べる「ICH は **normal character attribute** で空白を挿入する」に対し、
`insert_characters` は `pen.erase_cell()`（BCE）を使っている。**この逸脱の記録は
[`vt-conformance-scope.md`](vt-conformance-scope.md) §4 が持つ** — 参照実装の内訳も
terminfo の `bce` の出自もそちらにある。§4 が意図的な差異の正典で、この文書は
1回の洗い出しの作業記録なので、ここでは繰り返さず参照する。

IRM の経路では開いた桁が直後に glyph で上書きされるため、このバッチのどのケースにも
影響しない。

## 見直しの記録（2026-09-11）

初版の TC-06 は、主張2つは6実装すべてと一致していたが、テストがそのどちらも
識別できていなかった。

1. **行全体を同じ属性（BOLD/bg1）で塗っていた。** 文字だけ動かして属性を据え置く
   実装でも桁2は `'b'`+BOLD/bg1 になり、assert が通ってしまう。→ シフト元の
   セルに互いに違う背景を与えるよう変更。
2. **2つ目の pen が fg 既定・style 空だった。** `Pen::erase_cell` と `Pen::stamp` の
   結果が glyph 以外同一になり、「erase セルの `.c` を差し替えるだけ」の実装でも
   通ってしまう。→ pen に非既定の `fg` と非空の `style` を持たせるよう変更。
3. **「開いた桁を erase セルで埋めてから上書きする順序が保たれる」という説明は
   誤りだった。** 埋めは glyph に完全に覆われるので assert からは見えない。
   実際 alacritty・foot・wezterm の IRM 経路は埋めない。→ 削除し、埋めの形は
   `insert_characters` 側のテストが pin していると書き直した。

**TC-05（スクロールを伴う折り返しで Full を返す）は削除した。** `/simplify` の
reuse と simplification が独立に、既存の `a_wrap_on_the_bottom_row_scrolls` と
モード引数以外が同一だと指摘した。折り返しは shift より先に解決され、shift は
スクロール直後の空行に効き、damage の tail は `wrap` しか読まないので、insert と
replace で結果が一致しない経路が無い。当初からこの文書自身が Low・削除可と
記していたもの。insert 固有の順序は TC-04 が pin している。

あわせて付録「仕様にないもの」の「行に限られるかはマニュアルが決めていない」を
訂正し、C10 として繰り上げた。TC-01・TC-02・TC-03・TC-04・TC-07・TC-08 は
変更なし。TC-03・TC-07 は xterm（`ScrnInsertChar` の `MemMove` ループが0回に潰れ、
`ClearCells` が1セル消し、glyph が上書きする）と alacritty
（`column + width < columns` のガードがシフトを丸ごと飛ばす）で裏取りでき、TC-04 の
順序も xterm・ghostty・foot と一致することを確認した。

## docs/todo との衝突

無し。`vt-conformance-scope.md` は §1-A で `CSI 4 h/l` を `CSI∅`、DECAWM を
`MODE∅` と記録し、§5 で「次は IRM」と書いていて、この run の前提と一致する。
競合する API 提案は `docs/todo/` のどこにも無い。
