# Test cases: DynamicColorRequest::parse

`crates/orzma_vt/src/interpreter/osc/dynamic_color.rs` の
`DynamicColorRequest::parse` が負うテストケース。仕様は
`docs/references/xterm-ctlseqs.pdf` から導出した（DEC 系 3 冊と ECMA-48 は
"dynamic color" / "text foreground" のいずれも 0 hits で、この制御機能を扱って
いない）。citation は 12/12 検証済み。Phase 4 で著者が一覧を承認した。

signature の変更は提案していない。`parse(params: &[&[u8]]) -> Vec<Self>` は
13 件すべてを表現できる。

**このドキュメントの Rust はビルドしていない。** 転記して `cargo test` を通すのは
読み手の仕事で、この一覧は仕様由来の自己検証にとどまる。citation を 1〜2 本
自分で引き直してから残りを信用してほしい。現状の木に対して**コンパイルできる**
（`DynamicColorRequest` と `DynamicColor` は既に定義されており、`parse` は
部分実装の状態にある）。

## テストケース一覧

High → Medium の順に並べてある。High だけ転記して止めても、仕様が明文で述べる
挙動はすべて押さえられる。

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `an_osc_10_spec_decodes_to_a_foreground_set` | C4 | High |
| TC-02 | `an_osc_11_spec_decodes_to_a_background_set` | C5 | High |
| TC-03 | `a_question_mark_decodes_to_a_query` | C7 | High |
| TC-04 | `successive_values_change_successive_colors` | C2, C4, C5 | High |
| TC-05 | `a_chained_query_decodes_to_one_query_per_color` | C2, C7 | High |
| TC-06 | `a_chain_stops_at_the_first_color_this_terminal_does_not_carry` | C2, C6 | High |
| TC-07 | `an_osc_12_decodes_to_nothing` | C6 | High |
| TC-08 | `a_command_without_a_value_decodes_to_nothing` | C1 | High |
| TC-09 | `an_osc_110_decodes_to_a_foreground_reset` | C8 | High |
| TC-10 | `an_osc_111_decodes_to_a_background_reset` | C9 | High |
| TC-11 | `a_reset_ignores_the_parameters_after_its_number` | C8, C10 | Medium |
| TC-12 | `the_indexed_palette_commands_decode_to_no_dynamic_color_request` | C11, C12 | Medium |
| TC-A1 | `an_unreadable_spec_drops_only_its_own_position` | PI | Medium |

Source タグ — **C1**: p.39 L2102（Pt に最低 1 つのパラメータ）／**C2**: p.39
L2103（連鎖と開始点）／**C4**: p.40 L2133（OSC 10 = 前景）／**C5**: p.40
L2134（OSC 11 = 背景）／**C6**: p.40 L2135（OSC 12 = カーソル）／**C7**: p.40
L2126（`?` は問い合わせ）／**C8**: p.42 L2274（OSC 110）／**C9**: p.42
L2275（OSC 111）／**C10**: p.42 L2243（OSC 104 はパラメータを取る）／**C11**:
p.39 L2094（10〜19 が dynamic colors）／**C12**: p.39 L2098（ANSI colors とは
別物）／**PI**: `osc/palette.rs:35`。全エントリは付録の契約表にある。

TC-01 から TC-12 までは仕様由来。**TC-A1 だけが仕様由来ではない** —
マニュアルは読めない `color_spec` に何も言わず、この挙動は姉妹デコーダ
`PaletteRequest` の invariant から持ち込んでいる（下の TC-A1 を参照）。

テストコードは `crates/orzma_vt/src/interpreter/osc/dynamic_color.rs` の
`#[cfg(test)] mod tests` へ追加する。このファイルは新設なので既存のテスト
モジュールは無く、`fn rgb(r: u8, g: u8, b: u8) -> Rgb` ヘルパを
`osc/palette.rs` の同名ヘルパに倣って置く。`parse` は純関数なので、どのケース
にも setup は要らない。

## TC-01 — `OSC 10` の spec は前景の設定になる

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"10", b"rgb:12/34/56"])` |
| Expect | `[Set { target: Foreground, color: Rgb { r: 0x12, g: 0x34, b: 0x56 } }]` **[C4]** |

C4 の nominal ケース。`rgb:` 形式は C3 が `XParseColor` に委ねると述べる 2 形式
のうち terminfo の `initc` が実際に送るほうで、値の解釈自体は
`Rgb::from_color_spec` が `device/color.rs` のテストで押さえている。ここが見て
いるのは「10 という番号が前景に写る」ことだけ。

```rust
/// Asserts that an `OSC 10` carrying a colour spec decodes to a set of
/// the foreground.
///
/// Case: a colour-scheme script recolors the terminal's text with
/// `OSC 10 ; rgb:12/34/56`.
#[test]
fn an_osc_10_spec_decodes_to_a_foreground_set() {
    assert_eq!(
        DynamicColorRequest::parse(&[b"10", b"rgb:12/34/56"]),
        vec![DynamicColorRequest::Set {
            target: DynamicColor::Foreground,
            color: rgb(0x12, 0x34, 0x56)
        }]
    );
}
```

## TC-02 — `OSC 11` の spec は背景の設定になる

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"11", b"#202020"])` |
| Expect | `[Set { target: Background, color: Rgb { r: 0x20, g: 0x20, b: 0x20 } }]` **[C5]** |

C5 の nominal ケース。spec に `#` 形式を使うのは、C3 が委ねる 2 形式の残り
半分をこの層でも一度は通すため。`#202020` は上位ビット詰めなので
`rgb:20/20/20` と同じ値になる数少ない組で、丸め規則の差がこのケースの結果を
揺らさない。

```rust
/// Asserts that an `OSC 11` carrying a colour spec decodes to a set of
/// the background.
///
/// Case: a colour-scheme script gives the terminal a dark grey ground
/// with `OSC 11 ; #202020`.
#[test]
fn an_osc_11_spec_decodes_to_a_background_set() {
    assert_eq!(
        DynamicColorRequest::parse(&[b"11", b"#202020"]),
        vec![DynamicColorRequest::Set {
            target: DynamicColor::Background,
            color: rgb(0x20, 0x20, 0x20)
        }]
    );
}
```

## TC-03 — `?` は問い合わせになる

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"11", b"?"])` |
| Expect | `[Query { target: Background }]` **[C7]** |

C7 は「名前や RGB 指定ではなく `?` が与えられたとき」という条件文で、条件を
満たさない側は TC-01 と TC-02 が押さえている。ここは満たす側。`?` を
`Rgb::from_color_spec` に素通しする実装は `None` を受けて要求ごと落とすので、
問い合わせが黙って消える。

```rust
/// Asserts that a `?` in place of the spec decodes to a query of that
/// color.
///
/// Case: nvim asks for the background at startup so that it can decide
/// whether to set its `background` option to dark or light.
#[test]
fn a_question_mark_decodes_to_a_query() {
    assert_eq!(
        DynamicColorRequest::parse(&[b"11", b"?"]),
        vec![DynamicColorRequest::Query {
            target: DynamicColor::Background
        }]
    );
}
```

## TC-04 — 後続の値は後続の色を変える

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"10", b"rgb:ff/ff/ff", b"rgb:00/00/00"])` |
| Expect | `[Set { Foreground, #ffffff }, Set { Background, #000000 }]`（この順） **[C2][C4][C5]** |

C2 の連鎖規則そのもの。OSC 4 は「番号; spec」の組を繰り返すが、こちらは
「開始番号 + 値の並び」で形が違う。`[b"10", spec]` の 2 要素マッチだけで書いた
実装はこのコマンドにまったく反応しないので、そこを塞ぐ。順序も固定する —
要求は現れた順に適用され、同じ色を 2 回名指すコマンドでは後勝ちになる。

```rust
/// Asserts that the values after the first change each successive
/// color, so a command starting at `OSC 10` sets the foreground and
/// then the background.
///
/// Case: a theme script recolors both the text and the ground of the
/// terminal in a single command.
#[test]
fn successive_values_change_successive_colors() {
    assert_eq!(
        DynamicColorRequest::parse(&[b"10", b"rgb:ff/ff/ff", b"rgb:00/00/00"]),
        vec![
            DynamicColorRequest::Set {
                target: DynamicColor::Foreground,
                color: rgb(0xff, 0xff, 0xff)
            },
            DynamicColorRequest::Set {
                target: DynamicColor::Background,
                color: rgb(0x00, 0x00, 0x00)
            },
        ]
    );
}
```

## TC-05 — 連鎖した問い合わせは色ごとに 1 件になる

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"10", b"?", b"?"])` |
| Expect | `[Query { Foreground }, Query { Background }]`（この順） **[C2][C7]** |

C7 が "xterm can make more than one reply" と述べる根拠がここにある。応答を
2 本書くには、デコード段でまず 2 件に割れていなければならない。連鎖 (C2) と
問い合わせ (C7) が独立に効くことを、設定側 (TC-04) とは別に押さえる。

```rust
/// Asserts that a chained query decodes to one query per color.
///
/// Case: a program saves both the text and the ground colour before it
/// recolors them, so that it can restore each on exit.
#[test]
fn a_chained_query_decodes_to_one_query_per_color() {
    assert_eq!(
        DynamicColorRequest::parse(&[b"10", b"?", b"?"]),
        vec![
            DynamicColorRequest::Query {
                target: DynamicColor::Foreground
            },
            DynamicColorRequest::Query {
                target: DynamicColor::Background
            },
        ]
    );
}
```

## TC-06 — 連鎖はこの端末が持たない色で止まる

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"10", b"rgb:ff/00/00", b"rgb:00/ff/00", b"rgb:00/00/ff"])` |
| Expect | 前景と背景の 2 件だけを返し、3 つ目の値からは何も出さない **[C2][C6]** |

C2 により 3 つ目の値は位置 12、C6 によりそれは text cursor color で、orzma は
それを持たない。境界ケースなので High に置いてある。開始番号に添字を足して
`DynamicColor` に写す実装は、12 を素通しすると前景か背景のどちらかに化ける
危険があり、このケースがそれを止める。

```rust
/// Asserts that a chain stops at the first color this terminal does not
/// carry, so a third value reaches no cursor color.
///
/// Case: a script written for xterm recolors the text, the ground, and
/// the cursor in one command.
#[test]
fn a_chain_stops_at_the_first_color_this_terminal_does_not_carry() {
    assert_eq!(
        DynamicColorRequest::parse(&[
            b"10",
            b"rgb:ff/00/00",
            b"rgb:00/ff/00",
            b"rgb:00/00/ff"
        ]),
        vec![
            DynamicColorRequest::Set {
                target: DynamicColor::Foreground,
                color: rgb(0xff, 0x00, 0x00)
            },
            DynamicColorRequest::Set {
                target: DynamicColor::Background,
                color: rgb(0x00, 0xff, 0x00)
            },
        ]
    );
}
```

## TC-07 — `OSC 12` 単独は何も出さない

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"12", b"rgb:ff/00/00"])` |
| Expect | 空 **[C6]** |

TC-06 は連鎖の途中で範囲外に出る経路、こちらは開始点そのものが範囲外という
別の入口。C2 が「Ps が一覧の開始点を告げる」と述べるので、開始点が
この端末の持たない色なら、続く値のどれも行き先を持たない。

```rust
/// Asserts that an `OSC 12` decodes to nothing, its starting point
/// being a color this terminal does not carry.
///
/// Case: nvim recolors the cursor for insert mode through its
/// `guicursor` option.
#[test]
fn an_osc_12_decodes_to_nothing() {
    assert!(DynamicColorRequest::parse(&[b"12", b"rgb:ff/00/00"]).is_empty());
}
```

## TC-08 — 値を持たないコマンドは何も出さない

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"10"])` |
| Expect | 空 **[C1]** |

C1 "At least one parameter is expected for Pt" の下限側。番号だけのコマンドは
パーサから要素 1 つのスライスとして届くので、番号に一致した時点で既定色を
書きにいく実装だと、`OSC 10` 単体が前景を勝手に既定へ戻すことになる。

```rust
/// Asserts that a command carrying no value at all decodes to nothing.
///
/// Case: a program emits a bare `OSC 10`, omitting the separator and
/// the spec after it.
#[test]
fn a_command_without_a_value_decodes_to_nothing() {
    assert!(DynamicColorRequest::parse(&[b"10"]).is_empty());
}
```

## TC-09 — `OSC 110` は前景のリセットになる

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"110"])` |
| Expect | `[Reset { target: Foreground }]` **[C8]** |

C8 の nominal ケース。

```rust
/// Asserts that an `OSC 110` decodes to a reset of the foreground.
///
/// Case: a program restores the text colour it changed before it exits.
#[test]
fn an_osc_110_decodes_to_a_foreground_reset() {
    assert_eq!(
        DynamicColorRequest::parse(&[b"110"]),
        vec![DynamicColorRequest::Reset {
            target: DynamicColor::Foreground
        }]
    );
}
```

## TC-10 — `OSC 111` は背景のリセットになる

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"111"])` |
| Expect | `[Reset { target: Background }]` **[C9]** |

C9 の nominal ケース。

```rust
/// Asserts that an `OSC 111` decodes to a reset of the background.
///
/// Case: a program restores the ground colour it changed before it
/// exits.
#[test]
fn an_osc_111_decodes_to_a_background_reset() {
    assert_eq!(
        DynamicColorRequest::parse(&[b"111"]),
        vec![DynamicColorRequest::Reset {
            target: DynamicColor::Background
        }]
    );
}
```

## TC-11 — リセットは番号の後ろのパラメータを読み捨てる

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"110", b"11"])` |
| Expect | `[Reset { target: Foreground }]`。背景のリセットは含まない **[C8][C10]** |

根拠は 2 つの引用の**対比**で、単独の断定文ではないので Medium。C10 は
OSC 104 を `Ps = 1 0 4 ; c` と綴り "Any number of c parameters may be given" と
明示するのに対し、C8 と C9 は `Ps = 1 1 0` / `Ps = 1 1 1` とパラメータ無しで
綴られている。マニュアルが片方にだけパラメータを与えている以上、リセット側は
連鎖しない。OSC 104 のつもりで `OSC 110;11` を書くスクリプトが、背景まで
巻き添えにして戻さないことを固定する。

```rust
/// Asserts that a reset reads and discards the parameters after its
/// number, resetting only the color that number names rather than
/// chaining to the next one.
///
/// Case: a script restores both colours with `OSC 110 ; 11`, borrowing
/// the `OSC 104` spelling that does take colour numbers.
#[test]
fn a_reset_ignores_the_parameters_after_its_number() {
    assert_eq!(
        DynamicColorRequest::parse(&[b"110", b"11"]),
        vec![DynamicColorRequest::Reset {
            target: DynamicColor::Foreground
        }]
    );
}
```

## TC-12 — インデックスパレットのコマンドは dynamic color の要求にならない

| | |
| - | - |
| Setup | なし |
| Act | `parse(&[b"4", b"1", b"rgb:ff/00/00"])`、`parse(&[b"104"])`、`parse(&[b"0", b"title"])`、`parse(&[])` |
| Expect | すべて空 **[C11][C12]** |

C11 が dynamic colors を 10〜19 に限り、C12 が "They are not the same as the
ANSI colors" と明示的に切り分ける。2 つのデコーダは `osc_dispatch` で毎回
両方走るので、重なりが無いことをこちら側でも固定する。`osc/palette.rs` の
`other_commands_decode_to_no_palette_request` がこの対を反対側から押さえている。

```rust
/// Asserts that the indexed-palette commands and the window title
/// decode to no dynamic-color request.
///
/// Case: a theme script recolors an indexed slot and a shell sets its
/// title, and both reach this decoder on their way through the
/// operating system command dispatch.
#[test]
fn the_indexed_palette_commands_decode_to_no_dynamic_color_request() {
    assert!(DynamicColorRequest::parse(&[b"4", b"1", b"rgb:ff/00/00"]).is_empty());
    assert!(DynamicColorRequest::parse(&[b"104"]).is_empty());
    assert!(DynamicColorRequest::parse(&[b"0", b"title"]).is_empty());
    assert!(DynamicColorRequest::parse(&[]).is_empty());
}
```

## TC-A1 — 読めない spec はその位置だけ落ちる

| | |
| - | - |
| Setup | なし |
| Act | `DynamicColorRequest::parse(&[b"10", b"red", b"rgb:00/00/ff"])` |
| Expect | `[Set { Background, #0000ff }]`。前景は落ちるが、その後ろの位置は decode される **[PI]** |

**仕様由来ではないので `TC-A` 番号を振ってある。** マニュアルは C3 で
`color_spec` を `XParseColor` に委ねるだけで、読めない spec に何が起きるかを
述べない。実装も割れていて、xterm は最初の誤りで打ち切り、alacritty は
その位置を飛ばして続ける。orzma は OSC 4 で後者を選んでおり、その決定が
`PaletteRequest::parse` の `# Invariants` に文章として残っているので、姉妹
デコーダの一貫性としてこちらにも持ち込む。`red` を選んだのは、xterm が受ける
色名を orzma が意図的に受けないためで、実在するスクリプトが投げうる入力に
なっている。

```rust
/// Asserts that a spec that cannot be read drops only its own position
/// rather than ending the command, so the positions after it still
/// decode.
///
/// Case: a script written for xterm names the foreground `red`, a
/// colour name this terminal does not know, before giving the
/// background an `rgb:` spec.
#[test]
fn an_unreadable_spec_drops_only_its_own_position() {
    assert_eq!(
        DynamicColorRequest::parse(&[b"10", b"red", b"rgb:00/00/ff"]),
        vec![DynamicColorRequest::Set {
            target: DynamicColor::Background,
            color: rgb(0x00, 0x00, 0xff)
        }]
    );
}
```

## 付録

### 契約表

すべて `docs/references/xterm-ctlseqs.pdf`。行番号は `pdftotext -layout` での値。

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C1 | OSC 10–19 | bounded value | At least one parameter is expected for Pt. | p.39, L2102-2103 |
| C2 | OSC 10–19 | numeric parameter | Each successive parameter changes the next color in the list. The value of Ps tells the starting point in the list. | p.39, L2103-2105 |
| C3 | OSC 10–19 | unconditional | The colors are specified by name or RGB specification as per XParseColor. | p.39, L2105-2106 |
| C4 | OSC 10 | unconditional | Change VT100 text foreground color to Pt. | p.40, L2133 |
| C5 | OSC 11 | unconditional | Change VT100 text background color to Pt. | p.40, L2134 |
| C6 | OSC 12 | unconditional | Change text cursor color to Pt. | p.40, L2135 |
| C7 | OSC 10–19 | conditional | If a "?" is given rather than a name or RGB specification, xterm replies with a control sequence of the same form which can be used to set the corresponding dynamic color. | p.40, L2126-2131 |
| C8 | OSC 110 | unconditional | Reset VT100 text foreground color. | p.42, L2274 |
| C9 | OSC 111 | unconditional | Reset VT100 text background color. | p.42, L2275 |
| C10 | OSC 104 | unconditional | Reset Color Number c. Any number of c parameters may be given. | p.42, L2243-2245 |
| C11 | OSC 10–19 | unconditional | The 10 colors (below) which may be set or queried using 1 0 through 1 9 are denoted dynamic colors | p.39, L2094-2096 |
| C12 | OSC 10–19 | unconditional | They are not the same as the ANSI colors (however, the dynamic text foreground and background colors are used when ANSI colors are reset using SGR 3 9 and 4 9, respectively). | p.39, L2098-2101 |

C11 は最初 `using 10 through 19` と記録して REJECTED（11/18 matched）になった。
マニュアルは数字を `1 0` と分かち書きしており、実テキストへ引用を直して再検証
した。

kind-2 台帳:

```
PI — crates/orzma_vt/src/interpreter/osc/palette.rs:35, PaletteRequest::parse
     "A pair or number that cannot be read is dropped on its own and
      the rest still decode."
     justifies: 読めない spec はその位置だけ落ち、後続の位置は decode される
```

この台帳エントリには但し書きが要る。引用は**姉妹デコーダ `PaletteRequest` の
invariant** であって `DynamicColorRequest` について述べた文ではない。承認済みの
設計が「OSC 4 で選んだ逸脱と揃える」としているため持ち込んでいる。Phase 4 で
この但し書きごと提示し、著者が承認した。

C3 は単独のケースを生まない。`color_spec` の解釈は `Rgb::from_color_spec` に
委ねられており、`device/color.rs` のテストが 2 形式を押さえている。この層では
TC-01 が `rgb:`、TC-02 が `#` を通すことで委譲が生きていることを確かめる。

### 探索した語

| Level | Source | 語 | 結果 |
| - | - | - | - |
| 1 | `parse` の `///` | dynamic color, operating system command | xterm-ctlseqs に到達 |
| 2 | `DynamicColorRequest` の `///` | OSC 10, OSC 11, OSC 110, OSC 111 | xterm-ctlseqs に到達 |
| 3 | ファイルの `//!` | dynamic foreground, background | xterm-ctlseqs に到達 |
| 4 | signature と型名 | color_spec, XParseColor, Color String | C3 経由で xlib.pdf へ委譲、独立のケースは無し |

DEC 系マニュアルの探索記録（1e が要求する、降格の根拠）:

| 語 | vt510 | vt220 | ECMA-48 |
| - | - | - | - |
| `OSC` | 33 hits（いずれも制御導入子の定義で、dynamic color は無い） | 1 | 8 |
| `Operating System Command` | 3 | 0 | 5 |
| `dynamic color` | 0 | 0 | 0 |
| `text foreground` | 0 | 0 | 0 |

3 冊とも dynamic colors を扱っていないので、1e のとおり `xterm-ctlseqs.pdf` を
統治マニュアルとした（DEC のランキングの下ではなく、外側にある）。

### 仕様にないもの

- 読めない `color_spec` に何が起きるか。C3 は `XParseColor` に委ねるだけで、
  不正な入力の扱いを述べない。TC-A1 がこの穴を kind-2 で埋めている。
- 1 本のコマンドが同じ色を 2 回名指したときの優先順位。TC-04 が「現れた順」を
  固定するが、マニュアルにこれを述べた文は無い。
- パーサのパラメータ上限。vtparse 0.7 の `MAX_OSC = 64` はこのクレートの制約で
  あって仕様ではない。連鎖が実用上 2 件で尽きるこのメソッドでは効かない。

### 仕様の矛盾

無し。

### API 改定

signature の提案は無し。ただし 1 件、**left as is** として記録する。

- **C6**（p.40 L2135、"Change text cursor color to Pt."）は検証済みだが、
  否定形でしか served されていない。TC-06 と TC-07 が「orzma は持たないので
  落とす」ことを固定するだけで、カーソル色を設定するケースは無い。これは
  承認済みのスコープ決定（OSC 12 / 112 は今回見送り）どおりで、`Palette` に
  カーソル色のフィールドを足し、`TerminalParams` と WGSL の `paint_cursor` まで
  通す作業が別途要る。

### 出典なしの改善提案

無し。

### docs/todo との衝突

- `docs/todo/vt-conformance-scope.md:242` が「OSC 4 で入れた `PaletteRequest` を
  広げて扱う」と提案している。この一覧は承認済みの設計に従い、別型
  `DynamicColorRequest` を別モジュール `osc/dynamic_color.rs` に置く前提で
  書かれている。1 つの操作に 2 つの設計案が並んでいる状態で、**解決はしていない**
  — 実装が入った時点で同ファイルの §2 表と §6 を更新するときに、著者が決着させる
  こと。
