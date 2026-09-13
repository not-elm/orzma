# Test cases: Executor::apply_dynamic_color_requests

`crates/orzma_vt/src/interpreter.rs` の
`Executor::apply_dynamic_color_requests` が負うテストケース。契約は前 2 回の
ラン（`tdd-dynamic_color-parse.md` と `tdd-dynamic_color-dynamic_color_reply.md`）
で検証した `docs/references/xterm-ctlseqs.pdf` のエントリを再利用しており、
再検証はしていない。Phase 4 で著者が一覧を承認し、重複候補 2 件
（終端のエコー、`OSC 12`）を落として 9 件に絞った。TC-I12 だけはコードレビューの
指摘を受けて後から加わり、著者が事後に承認した。

signature の変更は提案していない。
`apply_dynamic_color_requests(&mut self, params: &[&[u8]])` は
`apply_palette_requests` と同型。

**このドキュメントの Rust はビルドしていない。** 書かれた時点では
`DeviceState` の `set_foreground_color` / `set_background_color` /
`reset_foreground_color` / `reset_background_color` が無くコンパイルできなかった
が、このケース群がそれらを駆動したので、現在の木に対しては通る。

## テストケース一覧

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-I1 | `an_osc_11_set_reaches_the_palette_background` | C5 | High |
| TC-I2 | `an_osc_10_set_reaches_the_palette_foreground` | C4 | High |
| TC-I3 | `a_recolor_marks_the_chunk_damaged_and_carries_the_palette` | PR, C12 | High |
| TC-I4 | `a_recolor_to_the_current_color_leaves_the_chunk_undamaged` | PR | High |
| TC-I5 | `a_query_is_answered_with_the_color_the_palette_holds` | C7 | High |
| TC-I6 | `a_chained_query_is_answered_once_per_color` | C2, C7 | High |
| TC-I8 | `an_osc_110_restores_the_default_foreground` | C8 | High |
| TC-I9 | `an_osc_111_restores_the_default_background` | C9 | High |
| TC-I10 | `a_set_and_a_query_in_one_command_apply_in_order` | C2, OQ | Medium |
| TC-I12 | `a_query_leaves_the_chunk_undamaged` | PR, C7 | Medium |

Source タグ — **C2**: xterm p.39 L2103（連鎖と開始点）／**C4**: p.40 L2133／
**C5**: p.40 L2134／**C7**: p.40 L2126／**C8**: p.42 L2274／**C9**: p.42 L2275／
**C12**: p.39 L2098（ANSI colors とは別だが、SGR 39 / 49 のセルはこれを引く）／
**PR** と **OQ**: `interpreter.rs:612-616`（下の台帳）。

10 件すべてが仕様由来か、既存メソッドの明文化された契約由来で、`TC-A` は無い。

**TC-I12 はゲートの後に加わった。** コードレビューが、`interpreter/tests/palette.rs` の
`a_query_leaves_the_chunk_undamaged` が OSC 4 について持つ契約の対がこちらに無いことを
指摘して追加し、著者が事後に承認した。

テストコードは `crates/orzma_vt/src/interpreter/tests/dynamic_colors.rs` を新設
して置き、`crates/orzma_vt/src/interpreter/tests.rs` に `mod dynamic_colors;` を
追加する。既存モジュールに OSC 10 / 11 を扱うものは無い（`palette` は
OSC 4 / 104 専用）。`interpret` / `replies_of` / `damage_of` / `Session` は
`tests.rs` の既存ヘルパをそのまま使う。

## 落としたケース

著者の判断で 2 件を落とした。どちらも下位層のテストが同じ契約を押さえている。

- **終端のエコー**（`ESC ] 11 ; ? ESC \` に `ESC \` で答える、**[D1]**）—
  `dynamic_color_reply` の `an_st_closed_query_is_answered_with_the_seven_bit_string_terminator`
  が書式側を、`interpreter/tests/palette.rs` の
  `a_query_closed_by_st_is_answered_with_st` が `current_byte` 経路を押さえる。
- **`OSC 12` が何も変えず何も答えない**（**[C6]**）—
  `DynamicColorRequest::parse` の `an_osc_12_decodes_to_nothing` が押さえる。

## TC-I1 — `OSC 11` の設定が palette の背景に届く

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b]11;rgb:20/20/20\x07")` |
| Expect | `device.palette().background == Rgb { r: 0x20, g: 0x20, b: 0x20 }` **[C5]** |

デコーダが `Set { Background, … }` を出しても、それを `Palette` に書く経路が
無ければ画面は変わらない。`osc_dispatch` からの呼び出しと `DeviceState` の
委譲が生きていることを見る。

```rust
/// Asserts that an `OSC 11` writes the colour it carries to the
/// palette's background.
///
/// Case: a colour-scheme script gives the terminal a dark grey ground
/// before the user's prompt draws.
#[test]
fn an_osc_11_set_reaches_the_palette_background() {
    let device = interpret(b"\x1b]11;rgb:20/20/20\x07");
    assert_eq!(
        device.palette().background,
        Rgb {
            r: 0x20,
            g: 0x20,
            b: 0x20
        }
    );
}
```

## TC-I2 — `OSC 10` の設定が palette の前景に届く

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b]10;rgb:12/34/56\x07")` |
| Expect | `device.palette().foreground == Rgb { r: 0x12, g: 0x34, b: 0x56 }` **[C4]** |

TC-I1 の前景側。2 つのフィールドは別なので、片方だけ配線した実装を捕まえる。

```rust
/// Asserts that an `OSC 10` writes the colour it carries to the
/// palette's foreground.
///
/// Case: a colour-scheme script recolors the terminal's text.
#[test]
fn an_osc_10_set_reaches_the_palette_foreground() {
    let device = interpret(b"\x1b]10;rgb:12/34/56\x07");
    assert_eq!(
        device.palette().foreground,
        Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56
        }
    );
}
```

## TC-I3 — 色が変われば chunk が damaged になり、フレームが palette を運ぶ

| | |
| - | - |
| Setup | なし |
| Act | `Session` に `\x1b]11;rgb:20/20/20\x07` を食わせ、続けてフレームを取る |
| Expect | chunk が damaged **[PR]** ／ フレームの `palette` が新しい背景を運ぶ **[C12]** |

C12 が「SGR 39 / 49 で描かれたセルは dynamic の前景・背景を引く」と定めるので、
1 色の変更が画面全体に及ぶ。`damage_of` は bool しか返さず `DamageSpan::Full`
と部分 damage を区別できないため、**フレームが palette を運ぶこと**を第 2 の
assert に置いて、変更がレンダラまで届くことを観測している。

```rust
/// Asserts that a recolor marks its chunk damaged and hands the new
/// palette to the frame.
///
/// Case: a theme script gives the terminal a dark grey ground, and
/// every cell drawn with the default background has to be repainted
/// against it.
#[test]
fn a_recolor_marks_the_chunk_damaged_and_carries_the_palette() {
    let mut session = Session::new();
    assert!(session.feed(b"\x1b]11;rgb:20/20/20\x07").damaged);
    let frame = session.frame().expect("a recolor owes a frame");
    assert_eq!(
        frame
            .palette
            .expect("a recolor owes the frame its palette")
            .background,
        Rgb {
            r: 0x20,
            g: 0x20,
            b: 0x20
        }
    );
}
```

## TC-I4 — 現在の色への再設定は chunk を damaged にしない

| | |
| - | - |
| Setup | なし |
| Act | `damage_of(b"\x1b]11;rgb:00/00/00\x07")`（既定の背景と同じ黒） |
| Expect | `false` **[PR]** |

`Palette::set_background` の戻り値を捨てて無条件に stage する実装だと、
プロンプトごとにテーマを送り直すシェルで毎回全画面が再描画される。
`osc/palette.rs` の `setting_a_slot_to_its_current_color_reports_no_change` が
下位で同じ契約を持ち、こちらはその戻り値が実際に使われていることを見る。

```rust
/// Asserts that a recolor to the colour the palette already holds
/// leaves the chunk undamaged rather than repainting.
///
/// Case: a shell prompt re-applies the same theme before every command
/// it runs.
#[test]
fn a_recolor_to_the_current_color_leaves_the_chunk_undamaged() {
    assert!(!damage_of(b"\x1b]11;rgb:00/00/00\x07"));
}
```

## TC-I5 — 問い合わせは palette が持つ色で答える

| | |
| - | - |
| Setup | なし |
| Act | `replies_of(b"\x1b]11;?\x07")` |
| Expect | `b"\x1b]11;rgb:0000/0000/0000\x07"` **[C7]** |

`dynamic_color_reply` が正しい形を組み立てても、`osc_dispatch` から呼ばれて
`reply` に届かなければ nvim は何も受け取らず、`background` を既定の `dark` の
まま残す。配線そのものを見るケース。既定の背景は黒なので、期待値は全チャンネル
ゼロになる。

```rust
/// Asserts that a query is answered with the colour the palette holds
/// at that point.
///
/// Case: nvim asks for the background at startup so that it can decide
/// whether to set its `background` option to dark or light.
#[test]
fn a_query_is_answered_with_the_color_the_palette_holds() {
    assert_eq!(replies_of(b"\x1b]11;?\x07"), b"\x1b]11;rgb:0000/0000/0000\x07");
}
```

## TC-I6 — 連鎖問い合わせは色ごとに 1 本ずつ答える

| | |
| - | - |
| Setup | なし |
| Act | `replies_of(b"\x1b]10;?;?\x07")` |
| Expect | `b"\x1b]10;rgb:ffff/ffff/ffff\x07\x1b]11;rgb:0000/0000/0000\x07"`（この順） **[C2][C7]** |

C7 の "xterm can make more than one reply" が成り立つのはここ。既定の前景は
白、背景は黒なので、2 本の応答は値でも区別がつく。1 本にまとめる実装や、
2 本目まで前景を答える実装を捕まえる。

```rust
/// Asserts that a chained query is answered once per color, each reply
/// naming its own colour number, in the order the query asked.
///
/// Case: a program saves both the text and the ground colour with a
/// single command before it recolors them.
#[test]
fn a_chained_query_is_answered_once_per_color() {
    assert_eq!(
        replies_of(b"\x1b]10;?;?\x07"),
        b"\x1b]10;rgb:ffff/ffff/ffff\x07\x1b]11;rgb:0000/0000/0000\x07"
    );
}
```

## TC-I8 — `OSC 110` が既定の前景へ戻す

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b]10;rgb:12/34/56\x07\x1b]110\x07")` |
| Expect | `device.palette().foreground == Palette::default().foreground`、背景は触られない **[C8]** |

```rust
/// Asserts that an `OSC 110` returns the foreground to its default and
/// leaves the background alone.
///
/// Case: a program restores the text colour it changed before it exits,
/// while the ground colour a theme script set stays as it is.
#[test]
fn an_osc_110_restores_the_default_foreground() {
    let device = interpret(b"\x1b]10;rgb:12/34/56\x07\x1b]11;rgb:20/20/20\x07\x1b]110\x07");
    assert_eq!(device.palette().foreground, Palette::default().foreground);
    assert_eq!(
        device.palette().background,
        Rgb {
            r: 0x20,
            g: 0x20,
            b: 0x20
        }
    );
}
```

## TC-I9 — `OSC 111` が既定の背景へ戻す

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b]10;…\x07\x1b]11;…\x07\x1b]111\x07")` |
| Expect | `device.palette().background == Palette::default().background`、前景は触られない **[C9]** |

TC-I8 の背景側。リセットを片方だけ配線した実装、および 110 と 111 を取り違えた
実装を捕まえる。

```rust
/// Asserts that an `OSC 111` returns the background to its default and
/// leaves the foreground alone.
///
/// Case: a program restores the ground colour it changed before it
/// exits, while the text colour a theme script set stays as it is.
#[test]
fn an_osc_111_restores_the_default_background() {
    let device = interpret(b"\x1b]10;rgb:12/34/56\x07\x1b]11;rgb:20/20/20\x07\x1b]111\x07");
    assert_eq!(device.palette().background, Palette::default().background);
    assert_eq!(
        device.palette().foreground,
        Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56
        }
    );
}
```

## TC-I10 — 1 本の中で設定と問い合わせが混在しても順に適用される

| | |
| - | - |
| Setup | なし |
| Act | `\x1b]10;rgb:ff/00/00;?\x07` を 1 本として食わせる |
| Expect | 前景が赤になる **[C2]** ／ 2 つ目の値が背景の問い合わせとして答えられる **[OQ]** |

C2 により 1 つ目の値は前景の設定、2 つ目は背景。要求をいったん全部集めてから
種類ごとにまとめて処理する実装だと、順序が崩れて問い合わせが設定より先に
答えられる。OQ が「現れた順に適用し、問い合わせはその時点の色で答える」と
述べるのがこの契約。

```rust
/// Asserts that a set and a query in one command apply in the order
/// they appear.
///
/// Case: a theme script recolors the text and asks for the ground
/// colour in the same command, so that it can restore the ground on
/// exit.
#[test]
fn a_set_and_a_query_in_one_command_apply_in_order() {
    let mut session = Session::new();
    let output = session.feed(b"\x1b]10;rgb:ff/00/00;?\x07");
    assert_eq!(output.replies, b"\x1b]11;rgb:0000/0000/0000\x07");
    assert_eq!(
        session.0.device.palette().foreground,
        Rgb {
            r: 0xff,
            g: 0x00,
            b: 0x00
        }
    );
}
```

## TC-I12 — 問い合わせは chunk を damaged にしない

| | |
| - | - |
| Setup | なし |
| Act | `damage_of(b"\x1b]11;?\x07")` |
| Expect | `false` **[PR][C7]** |

C7 の問い合わせは色を報告するだけで変えないので、PR の「色が変わったときだけ
stage する」に照らすと何も stage されない。`stage` を `if changed` の外に出す
変更や、`Query` の腕で `changed` を立てる変更が入ると、nvim の起動ごと
（毎回 `OSC 11;?` を投げる）に coalesce ウィンドウが開いて全画面が再描画される。
`interpreter/tests/palette.rs` の同名テストが OSC 4 について同じ契約を持つ。

```rust
/// Asserts that a query leaves the chunk undamaged, a reply owing no
/// repaint of its own.
///
/// Case: nvim probes the background at startup without recoloring it,
/// and the terminal must not open a coalesce window for a frame that
/// carries nothing new.
#[test]
fn a_query_leaves_the_chunk_undamaged() {
    assert!(!damage_of(b"\x1b]11;?\x07"));
}
```

## 付録

### 契約表

前 2 回のランで検証済みのエントリを再利用している。全文と行番号は
`tdd-dynamic_color-parse.md` と `tdd-dynamic_color-dynamic_color_reply.md` の
付録にある。

| ID | Statement | Citation |
| - | - | - |
| C2 | Each successive parameter changes the next color in the list. The value of Ps tells the starting point in the list. | xterm-ctlseqs.pdf p.39, L2103-2105 |
| C4 | Change VT100 text foreground color to Pt. | p.40, L2133 |
| C5 | Change VT100 text background color to Pt. | p.40, L2134 |
| C7 | If a "?" is given …, xterm replies with a control sequence of the same form which can be used to set the corresponding dynamic color. | p.40, L2126-2131 |
| C8 | Reset VT100 text foreground color. | p.42, L2274 |
| C9 | Reset VT100 text background color. | p.42, L2275 |
| C12 | They are not the same as the ANSI colors (however, the dynamic text foreground and background colors are used when ANSI colors are reset using SGR 3 9 and 4 9, respectively). | p.39, L2098-2101 |

kind-2 台帳:

```
PR — crates/orzma_vt/src/interpreter.rs:615, Executor::apply_palette_requests
     "A command that changes a slot stages one full repaint, whatever
      the number of requests it carries."
     justifies: 色が実際に変わったときだけ full repaint を1回 stage する

OQ — crates/orzma_vt/src/interpreter.rs:612, Executor::apply_palette_requests
     "Applies the palette requests an `OSC 4` or `OSC 104` carries, in
      order, answering each query with the slot's colour at that point."
     justifies: 要求は現れた順に適用され、問い合わせはその時点の色で答える
```

どちらも**姉妹メソッド `apply_palette_requests` の doc** であって、
`apply_dynamic_color_requests` について述べた文ではない。承認済みの設計が
「`apply_palette_requests` と同じ形」としているため持ち込んでいる。Phase 4 で
この但し書きごと提示し、著者が承認した。

### 探索した語

| Level | Source | 語 | 結果 |
| - | - | - | - |
| 1 | `apply_dynamic_color_requests` の `///` | OSC 10, OSC 11, OSC 110, OSC 111 | 前回までに検証済みの C2/C4/C5/C7/C8/C9 |
| 2 | 姉妹メソッド `apply_palette_requests` の `///` | full repaint, in order, at that point | kind-2 台帳の PR と OQ |
| 4 | signature の型（`DeviceState`, `Palette`） | SGR 39, SGR 49, default foreground/background | C12 |

### 仕様にないもの

- **damage の粒度。** マニュアルは damage を扱わない。full repaint にするのは
  C12 の帰結（既定色のセルが全部変わる）で、`DamageSpan::Full` という綴りは
  orzma の契約。
- **応答を 1 本の chunk にまとめて出すか、要求ごとに分けるか。** C7 は
  "more than one reply" としか言わない。TC-I6 は連結されたバイト列を期待して
  いるが、これは `replies` が 1 本のバッファである実装の都合。

### 仕様の矛盾

無し。

### API 改定

無し。ただし、このケース群は `DeviceState` に 4 つのメソッドを生やすことを
前提にしている（`set_foreground_color` / `set_background_color` /
`reset_foreground_color` / `reset_background_color`）。既存の
`set_indexed_color` と同じ薄い委譲で、専用テストは持たない — その形に
合わせるという判断は Phase 4 の前に著者が別途承認している。

### 出典なしの改善提案

無し。

### docs/todo との衝突

- `docs/todo/vt-conformance-scope.md:242` が「`PaletteRequest` を広げて扱う」と
  提案しており、別型 `DynamicColorRequest` を使うこの一覧と食い違う。
  `tdd-dynamic_color-parse.md` に記録済みで、**解決はしていない**。
