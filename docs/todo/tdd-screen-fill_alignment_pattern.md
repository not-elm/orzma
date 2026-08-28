# Test cases: Screen::fill_alignment_pattern

対象は `crates/orzma_vt/src/screen.rs:763` の `pub fn fill_alignment_pattern(&mut self) -> DamageSpan`（DECALN、`ESC # 8`）。参照した仕様は `docs/references/` の `vt510.pdf` と `vt220.pdf`。`ECMA-48.pdf` は DECALN を持たない（DEC 私的シーケンスのため）。**citations verified: 6/6**（マニュアル4件、リポジトリ2件）。

Phase 4 の結果: 7件すべて承認。TC-03 は自明として確認なしで確定。TC-A3 は doc コメントへの1文追記を前提として追加が承認された（**追記は作者の作業で、まだツリーに入っていない**）。

**この Rust は現状のツリーに対してコンパイルできる。** シグネチャは既に存在し、本体は `todo!()` なので、全テストがそこで panic する。それが意図した RED である。TC-A3 の kind-2 典拠となる doc の1文だけはツリーに無いが、これはコンパイルではなく引用の問題である。

**この文書の Rust はビルドも実行もされていない。** 貼って動くものではなく、転記して RED を確認するための提案である。

レビューは入っていない。仕様から導出し自己検証しただけなので、信用する前に citation を1つ2つ抜き取りで確かめてほしい。

## テストケース一覧

| #     | 名前                                                | Source   | 優先度 |
| ----- | --------------------------------------------------- | -------- | ------ |
| TC-01 | `an_alignment_pattern_fills_every_visible_cell`      | C1, C4   | High   |
| TC-02 | `an_alignment_pattern_widens_the_margins_to_the_page`| C2       | High   |
| TC-03 | `an_alignment_pattern_seats_the_cursor_at_home`      | C3       | High   |
| TC-04 | `an_alignment_pattern_disarms_the_deferred_wrap`     | C3, SC   | Medium |
| TC-A1 | `an_alignment_pattern_returns_the_origin_to_the_corner` | AO    | Medium |
| TC-A2 | `an_alignment_pattern_leaves_the_history_untouched`  | AO       | Medium |
| TC-A3 | `an_alignment_pattern_ignores_the_pen`               | AD       | Medium |

Source タグ — **C1**: vt510.pdf p.124 L4443（complete screen area を test pattern で埋める）／**C2**: vt510.pdf p.124 L4451（マージンをページ端へ）／**C3**: 同 L4451（カーソルをホームへ）／**C4**: vt220.pdf p.48 L2451（大文字 E で埋める）／**AO**: `screen.rs:757` の doc（可視画面・絶対原点）／**SC**: `screen.rs:208` の doc（カーソル着席で deferred wrap 解除）／**AD**: `screen.rs:757` へ追記予定の doc（既定属性で描く）

TC-01 から TC-04 まではマニュアルの記述が直接支える。**`TC-A` 番号の3件はマニュアル典拠を持たない**: DECOM 解除の一次典拠 DEC STD 070 は `docs/references/` に無く、スクロールバックは VT 実機に存在せず、表示属性については両マニュアルとも沈黙している。いずれもリポジトリ側の doc コメントだけが根拠である。

テストコードは `crates/orzma_vt/src/screen.rs` に **新規モジュール `mod fill_alignment_pattern`** を作って追加する。既存の `mod reset`（2466行）と `mod placements`（2662行）の間が、メソッド宣言順と一致する。`screen()`（4×3、履歴10）と `tall_screen()`（4×4、履歴10）は既存ヘルパをそのまま使う。

---

## TC-01 — 可視画面の全セルが `E` になり、全面 damage を返す

| | |
| - | - |
| Setup | `screen()`; `'x'` を1文字印字して塗り替えを観測可能にする |
| Act | `screen.fill_alignment_pattern()` |
| Expect | 全ビューポート行の全セルの `.c` が `'E'` **[C1][C4]** ／ 戻り値が `DamageSpan::Full` **[C1]** |

vt510 は "a test pattern" としか言わず文字を特定しないので、`'E'` は vt220 側の C4 が供給する。2つの契約エントリに跨るのはそのためである。戻り値は `Option<DamageSpan>` ではないので skill の Damage 表（kind 3 タグ）が使えず、「complete screen area が埋まる」という C1 の状態記述から `Full` を直接導いている。事前に文字を置くのは、空の画面を埋めても「何もしていない実装」と区別がつかないため。

```rust
/// Asserts that the alignment pattern reaches every visible cell and
/// reports the whole screen as damaged.
///
/// Case: a service technician sends `ESC # 8` to a terminal showing a
/// half-drawn prompt, to get a uniform field to judge the display
/// against.
#[test]
fn an_alignment_pattern_fills_every_visible_cell() {
    let mut screen = screen();
    screen.print('x');
    assert_eq!(screen.fill_alignment_pattern(), DamageSpan::Full);
    for line in 0..3 {
        let row = screen.viewport_row(ViewportLine(line));
        assert!(row.iter().all(|cell| cell.c == 'E'));
    }
}
```

## TC-02 — 事前に設定されたスクロール領域が消える

| | |
| - | - |
| Setup | `tall_screen()`; `screen.set_scroll_region(Some(2), Some(3))` |
| Act | `screen.fill_alignment_pattern()` |
| Expect | `scroll_region.top_margin() == ScreenLine(0)` **[C2]** ／ `scroll_region.bottom_margin() == ScreenLine(3)` **[C2]** |

4行の画面に `CSI 2 ; 3 r` を当てると領域は `ScreenLine(1)..=ScreenLine(2)` になり、**上下どちらの境界もページ端と異なる**。片方だけ直す実装がこれで落ちる。`tall_screen()` を使うのは、`screen()` の3行だと領域を取ると下端の余白が1行しか残らず、境界の差が読みにくいため。alacritty 方式（塗るだけ）との差分が現れるのがこのケースである。

```rust
/// Asserts that the alignment pattern returns the scrolling margins to
/// the extremes of the page.
///
/// Case: a full-screen application has reserved a status line with
/// `CSI 2 ; 3 r` when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_widens_the_margins_to_the_page() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.fill_alignment_pattern();
    assert_eq!(screen.scroll_region.top_margin(), ScreenLine(0));
    assert_eq!(screen.scroll_region.bottom_margin(), ScreenLine(3));
}
```

## TC-03 — カーソルがホームへ移る

| | |
| - | - |
| Setup | `screen()`; `screen.move_cursor_to(Some(2), Some(3))` |
| Act | `screen.fill_alignment_pattern()` |
| Expect | `state.line == ScreenLine(0)` かつ `state.column == GridColumn(0)` **[C3]** |

自明なケースとして確認なしで確定した3条件を満たす。契約の shape が unconditional で、setup がコンストラクタとカーソル配置だけで済み、`Expect:` が単一の契約エントリ C3 に閉じている。両軸を見るのは、行だけ戻して列を放置する実装を通さないため。

```rust
/// Asserts that the alignment pattern seats the cursor at the home
/// position.
///
/// Case: the cursor sits in the middle of the screen when `ESC # 8`
/// arrives.
#[test]
fn an_alignment_pattern_seats_the_cursor_at_home() {
    let mut screen = screen();
    screen.move_cursor_to(Some(2), Some(3));
    screen.fill_alignment_pattern();
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(0));
}
```

## TC-04 — armed な deferred wrap が解除される

| | |
| - | - |
| Setup | `screen()`; 4列すべてに印字して deferred wrap を armed にする |
| Act | `screen.fill_alignment_pattern()` |
| Expect | `state.pending_wrap == false` **[C3][SC]** |

C3（カーソルがホームへ移る）と SC（カーソルを座らせると wrap が解除される）の連鎖で正当化される。`seat_home` を経由せず `state.line` と `state.column` に直接代入する実装がこれで落ちる。4列の `screen()` で4文字印字すると wrap が armed になるのは既存の `a_tab_keeps_the_deferred_wrap_armed` が使っている手順と同じで、setup 前の `assert!` はその前提が崩れていないことを示すために置く。

```rust
/// Asserts that the alignment pattern disarms an armed deferred wrap.
///
/// Case: the cursor has just printed into the last column, leaving the
/// wrap pending, when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_disarms_the_deferred_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c);
    }
    assert!(screen.state.pending_wrap);
    screen.fill_alignment_pattern();
    assert!(!screen.state.pending_wrap);
}
```

## TC-A1 — origin mode が絶対原点へ戻る

| | |
| - | - |
| Setup | `tall_screen()`; `set_scroll_region(Some(2), Some(3))`; `scroll_region.set_origin_mode(OriginMode::WithinMargins)` |
| Act | `screen.fill_alignment_pattern()` |
| Expect | `scroll_region.origin_mode() == OriginMode::UpperLeftCorner` **[AO]** |

**マニュアル典拠を持たない。** vt510 の DECALN 注記はマージンとカーソルしか言及せず、DECOM 解除を規定する DEC STD 070 は `docs/references/` に無い。作者が書いた doc の "the absolute cursor origin" だけが根拠なので `TC-A` 番号を振っている。マージンを先に設定するのは、`WithinMargins` がページ全体と一致していると解除の有無を区別できないため。

```rust
/// Asserts that the alignment pattern returns the cursor origin to the
/// upper-left corner.
///
/// Case: an application has turned on origin mode inside a reserved
/// pane when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_returns_the_origin_to_the_corner() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen
        .scroll_region
        .set_origin_mode(OriginMode::WithinMargins);
    screen.fill_alignment_pattern();
    assert_eq!(
        screen.scroll_region.origin_mode(),
        OriginMode::UpperLeftCorner
    );
}
```

## TC-A2 — スクロールバックは塗られない

| | |
| - | - |
| Setup | `screen()`; `'a'` を印字し、`state.line` を最下行へ置いて `line_feed()` で履歴へ押し出す |
| Act | `screen.fill_alignment_pattern()` |
| Expect | 履歴に落ちた行の先頭セルの `.c` が `'a'` のまま **[AO]** |

**マニュアル典拠を持たない。** VT 実機にスクロールバックが無いため、両マニュアルとも何も言えない。doc の "visible screen" が唯一の根拠である。`Grid` 全体を舐める実装、あるいは `fill_visible_row_range` ではなく履歴を含む経路で塗る実装がこれで落ちる。塗った**後**に `viewport.offset` を動かすのは、DECALN が塗るのは表示中のスクロールバックではなく画面そのものだからで、履歴を覗きに行くのは検証のためだけである。手順は既存の `a_reset_drops_the_scrollback_history` と同じ。

```rust
/// Asserts that the alignment pattern leaves the scrollback history
/// untouched.
///
/// Case: an earlier command's output has scrolled off the top of the
/// screen when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_leaves_the_history_untouched() {
    let mut screen = screen();
    screen.print('a');
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    screen.fill_alignment_pattern();
    screen.viewport.offset = DisplayOffset(1);
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
}
```

## TC-A3 — 塗るセルは現在の pen ではなく既定属性を持つ

| | |
| - | - |
| Setup | `screen()`; `screen.pen_mut().bg` を既定以外へ設定し、1文字印字する |
| Act | `screen.fill_alignment_pattern()` |
| Expect | 全ビューポート行の全セルが `Cell { c: 'E', ..Cell::default() }` と等しい **[AD]** |

**マニュアル典拠を持たない**うえ、根拠となる doc の1文がまだツリーに無い（下の「API 改定」参照）。`erase_in_display` を流用して `pen.erase_cell()` で塗る実装を防ぐのが目的で、BCE 消去とは埋めるセルの出所が違うという一点を固定する。既存の `a_reset_leaves_no_trace_of_the_pen_background_in_the_cells` が `reset` に対して同じことをしており、比較対象を `Cell::default()` に取る書き方もそこから借りている。

```rust
/// Asserts that the alignment pattern is drawn with default attributes
/// rather than the current pen.
///
/// Case: an application has selected a red background for a banner and
/// has not restored the rendition when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_ignores_the_pen() {
    let mut screen = screen();
    screen.pen_mut().bg = Color::Indexed(1);
    screen.print('x');
    screen.fill_alignment_pattern();
    let expected = Cell {
        c: 'E',
        ..Cell::default()
    };
    for line in 0..3 {
        let row = screen.viewport_row(ViewportLine(line));
        assert!(row.iter().all(|cell| *cell == expected));
    }
}
```

---

## 付録

### 契約表

| ID | Governs | Shape | Statement | Citation |
| -- | ------- | ----- | --------- | -------- |
| C1 | DECALN | unconditional | This control function fills the complete screen area with a test pattern used for adjusting screen alignment. | vt510.pdf p.124, L4443-4444 |
| C2 | DECALN | unconditional | DECALN sets the margins to the extremes of the page | vt510.pdf p.124, L4451 |
| C3 | DECALN | unconditional | moves the cursor to the home position | vt510.pdf p.124, L4451 |
| C4 | DECALN | unconditional | This sequence fills the screen with uppercase E's. | vt220.pdf p.48, L2451 |

kind-2 台帳:

```
AO — crates/orzma_vt/src/screen.rs:757, Screen::fill_alignment_pattern
     "Fills the visible screen with the alignment pattern, returning to
      the page-wide scroll region and the absolute cursor origin."
     justifies: 塗る範囲は可視画面に限る／origin mode は絶対原点へ戻る

SC — crates/orzma_vt/src/screen.rs:208, Screen::seat_cursor
     "Seats the cursor at `line` — measured from the origin the current
      [`OriginMode`] defines — and `column`, clamping both axes and
      disarming the deferred wrap."
     justifies: ホームへ移すことで deferred wrap が解除される

AD — crates/orzma_vt/src/screen.rs:757, Screen::fill_alignment_pattern（追記予定・未反映）
     "The pattern is drawn with default attributes rather than the current
      pen, because a screen tinted by the application's colors is useless
      as an adjustment reference."
     justifies: 塗られるセルは pen の色・装飾を引き継がない
```

### 探索した語

Level 1（メソッドの `///` ブロック）で behaviour の記述に到達したため、Level 2 以降へは降りていない。

| 語 | Level | 結果 |
| -- | ----- | ---- |
| `DECALN` | 1（`# Control Functions`） | vt510 L4442-4451、vt220 L2443-2451 に到達。ECMA-48 は 0 件 |
| `ESC # 8` | 1（`# Control Functions`） | vt510 L4447、vt220 L2449 の Format 欄。behaviour の記述ではない |
| `alignment pattern` | 1（要約文 "the alignment pattern"） | vt220 L2445-2446 に到達 |
| `scroll region` → margins / page | 1（要約文 "page-wide scroll region"） | C2 に到達 |
| `cursor origin` → origin mode / DECOM | 1（要約文 "absolute cursor origin"） | **DECALN の記述内には到達せず。** vt510 の DECALN 節は origin mode に言及しない |
| `alignment`（ECMA-48） | 1 | TALE / TATE / TCC のみ。DECALN の記述ではないため 1d により棄却（3件） |

`DamageSpan` は Level 4 の対象だが、orzma 独自の型でありマニュアルに対応語を持たない。

### 仕様にないもの

- **origin mode（DECOM）の解除** — vt510 の DECALN 節は言及しない。一次典拠は DEC STD 070 Appendix D.8（1985）だが `docs/references/` に無く、この run では検証できない。kind-2 の AO で TC-A1 として拾っている。
- **スクロールバックを塗らないこと** — VT 実機にスクロールバックが存在しないため、両マニュアルとも規定しえない。kind-2 の AO で TC-A2 として拾っている。
- **既定属性で塗ること** — 両マニュアルとも表示属性に沈黙している。kind-2 の AD（未反映）で TC-A3 として拾っている。
- **pen・タブ・文字集合・checkpoint・履歴を維持すること** — マニュアルにも doc コメントにも記述が無く、admissible な典拠を持たないためケースにしていない。
- **行属性（DECDWL / DECDHL）を単幅単高へ戻すこと** — DEC STD 070 が列挙するが、上記のとおりローカル典拠が無い。かつ `docs/todo/esc-dispatch.md` の B-4 で行属性自体が見送られており、モデルする状態が存在しない。

### 仕様の矛盾

なし。vt510 が文字を特定せず vt220 が `E` を特定するのは、沈黙と規定であって不一致ではないため、1e の precedence は適用していない。

### API 改定

blocker は無かった。`-> DamageSpan`（`Option` なし）は、DECALN が常に complete screen area を埋める以上 C1 と矛盾せず、kind R には当たらない。

作者が Phase 4 で取った変更が1件ある。**これは signature ではなく doc コメントで、まだツリーに入っていない。**

```rust
/// Fills the visible screen with the alignment pattern, returning to
/// the page-wide scroll region and the absolute cursor origin.
///
/// The pattern is drawn with default attributes rather than the current
/// pen, because a screen tinted by the application's colors is useless
/// as an adjustment reference.
///
/// # Control Functions
///
/// - `DECALN` (`ESC # 8`)
```

この追記が kind-2 台帳の AD を成立させ、TC-A3 を可能にする。追記前は AD の quote が `grep -F` で見つからないため、TC-A3 は典拠を持たないケースになる。

### docs/todo との衝突

`docs/todo/esc-dispatch.md` の A-2 が、マージンとカーソルの扱いを**未決事項として**書いている。

> 実装前に決めること: VT510 は「DECALN はマージンをページの端まで広げ、カーソルを
> ホーム位置へ移す」と書いているが、alacritty の `decaln` はマージンにもカーソルにも
> 触れない。

この文書は VT510 準拠で確定したものとして TC-02 / TC-03 を導いており、A-2 の記述と食い違う。**解決はしない。** A-2 は提案であり、どちらを正とするかは作者の判断である。
