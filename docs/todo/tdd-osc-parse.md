# Test cases: PaletteRequest::parse

`crates/orzma_vt/src/interpreter/osc.rs:90` の `PaletteRequest::parse` が負うテストケースを、
`docs/references/xterm-ctlseqs.pdf`（OSC 4 / OSC 5 / OSC 104）から洗い出した。
citations verified: 13/13（マニュアル 10 件、リポジトリの doc comment 3 件）。
Phase 4 の結果: 13 件すべて承認（2026-09-12）。signature の変更提案は無し。

**このドキュメントの Rust は現状の木ではコンパイルは通るが、実行すると必ず失敗する。**
`parse` の本体はまだ `todo!()` のスタブなので、どのテストも panic で落ちる。それが実装計画の
RED ステップであり、本体を書いた時点で GREEN になる。**ビルド・実行は一度もしていない**。

このリストを点検したのは私だけで、レビューは受けていない。仕様由来であることと自己検証済みで
あることだけが根拠なので、信用する前に citation を 1〜2 件抜き取りで確かめてほしい。

## テストケース一覧

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `an_osc_4_pair_decodes_to_a_set` | D1 | High |
| TC-02 | `several_pairs_decode_in_order` | D2, D1 | High |
| TC-03 | `a_question_mark_decodes_to_a_query` | D4 | High |
| TC-04 | `each_query_in_one_command_decodes_separately` | D5 | High |
| TC-05 | `the_table_bounds_decode` | D3 | High |
| TC-06 | `a_color_number_past_the_table_is_dropped` | D3, P2 | High |
| TC-07 | `an_osc_104_number_decodes_to_a_reset` | D7 | High |
| TC-08 | `several_osc_104_numbers_reset_each_slot` | D8, D10 | High |
| TC-09 | `a_bare_osc_104_decodes_to_a_reset_of_every_slot` | D9 | High |
| TC-10 | `other_commands_decode_to_no_palette_request` | D1, D7 | High |
| TC-11 | `an_unpaired_trailing_number_is_ignored` | D2, P3 | High |
| TC-A1 | `an_unreadable_spec_drops_only_its_own_pair` | P1 | Medium |
| TC-A2 | `a_malformed_color_number_is_dropped` | P1 | Medium |

Source タグ — **D1**: p.38 L2059-2060（Change Color Number）／**D2**: p.38 L2062-2063（c/spec の組は
何組でも）／**D3**: p.38 L2063-2066（色番号は ANSI 0–7・bright 8–15・256 色表の残り）／**D4**: p.38
L2068-2070（`?` は設定と同じ形の応答を返す）／**D5**: p.38 L2070-2072（応答は複数返りうる）／
**D6**: p.39 L2077-2079（スペシャルカラーは OSC 4 に最大色数を足しても設定できる）／**D7**: p.42
L2243-2244（Reset Color Number）／**D8**: p.42 L2244-2245（番号は何個でも）／**D9**: p.42 L2248
（引数が無ければ表全体をリセット）／**D10**: p.42 L2245-2247（OSC 104 の番号も同じ表を指す）／
**P1**: osc.rs:84（読めない組・番号はそれだけ落として続行）／**P2**: osc.rs:85（256 以上は切り詰め
ずに落とす）／**P3**: osc.rs:88（相方の無い番号は無視）。全文は付録の契約表にある。

TC-01 から TC-11 までは仕様が明示している振る舞いとその境界で、マニュアルの文が根拠になっている。
TC-A1 と TC-A2 はリポジトリの doc comment しか根拠が無い（`TC-A` の番号がそれを表す）。優先度は
High → Medium の順に並べてあるので、High まで読めば仕様が明示している範囲は尽きる。

このメソッドの戻り値は `Vec<Self>` なので、`Option<DamageSpan>` 用の Damage 判定表は適用されない。
`Expect:` は返る `Vec` の中身をそのまま名指しする。

テストコードは `crates/orzma_vt/src/interpreter/osc.rs` の `mod tests`（line 164、サブモジュール無し
の平坦な構成）へ追加する。`Rgb` を組み立てるだけのヘルパを同じモジュールに置いて使う。

```rust
    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }
```

---

## TC-01 — 色 spec を伴う組は Set になる

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"4", b"1", b"rgb:12/34/56"])` |
| Expect | `vec![PaletteRequest::Set { index: 1, color: rgb(0x12, 0x34, 0x56) }]` を返す **[D1]** |

もっとも素直な 1 組。番号と spec が正しく対応づくこと、そして `Rgb` への変換が
`Rgb::from_color_spec` に委ねられていることを押さえる。

```rust
/// Asserts that an `OSC 4` pair carrying a colour spec decodes to a set
/// of that slot.
///
/// Case: terminfo's `initc` recolors slot 1 with
/// `OSC 4 ; 1 ; rgb:12/34/56`.
#[test]
fn an_osc_4_pair_decodes_to_a_set() {
    assert_eq!(
        PaletteRequest::parse(&[b"4", b"1", b"rgb:12/34/56"]),
        vec![PaletteRequest::Set {
            index: 1,
            color: rgb(0x12, 0x34, 0x56)
        }]
    );
}
```

## TC-02 — 複数の組は現れた順に並ぶ

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"4", b"1", b"rgb:ff/00/00", b"2", b"?", b"3", b"#00f"])` |
| Expect | `[Set { 1, (ff,00,00) }, Query { 2 }, Set { 3, (00,00,f0) }]` の順で返す **[D2]** ／ 各組は設定にも問い合わせにもなる **[D1]** |

マニュアルは組を何組でも許すので、設定と問い合わせが混在する 1 本がありうる。順序が観測可能な
契約であることを押さえる狙いで、番号をキーにした map へ集める実装や、設定と問い合わせを別々に
集めてから連結する実装はここで落ちる。`#00f` の青が 0xf0 になるのは `#` が上位ビット詰めだから
（`Rgb::from_color_spec` 側の契約）。

```rust
/// Asserts that several pairs in one command decode in the order they
/// appear, mixing sets and queries.
///
/// Case: a theme script recolors two slots and asks for a third in one
/// command.
#[test]
fn several_pairs_decode_in_order() {
    assert_eq!(
        PaletteRequest::parse(&[b"4", b"1", b"rgb:ff/00/00", b"2", b"?", b"3", b"#00f"]),
        vec![
            PaletteRequest::Set {
                index: 1,
                color: rgb(0xff, 0x00, 0x00)
            },
            PaletteRequest::Query { index: 2 },
            PaletteRequest::Set {
                index: 3,
                color: rgb(0x00, 0x00, 0xf0)
            },
        ]
    );
}
```

## TC-03 — spec の代わりの `?` は Query になる

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"4", b"1", b"?"])` |
| Expect | `vec![PaletteRequest::Query { index: 1 }]` を返す **[D4]** |

`?` は色名でも RGB 指定でもない特別扱いで、設定ではなく応答を求める。`?` を色文字列として
`Rgb::from_color_spec` に渡してしまう実装（結果として組ごと捨てる）を捕まえる。

```rust
/// Asserts that a `?` in place of the spec decodes to a query of that
/// slot.
///
/// Case: a program asks for slot 1 before recoloring it, so that it can
/// restore the slot on exit.
#[test]
fn a_question_mark_decodes_to_a_query() {
    assert_eq!(
        PaletteRequest::parse(&[b"4", b"1", b"?"]),
        vec![PaletteRequest::Query { index: 1 }]
    );
}
```

## TC-04 — 1 本の中の問い合わせはそれぞれ別に立つ

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"4", b"0", b"?", b"1", b"?"])` |
| Expect | `[Query { 0 }, Query { 1 }]` の 2 件を返す **[D5]** |

マニュアルは「1 本に複数の組を置けるので応答も複数返りうる」と明言している。応答を組み立てるのは
Executor だが、その前提として parse が問い合わせを 1 件に畳まず個別に返すことが要る。最初の
問い合わせだけを見て打ち切る実装を捕まえる。

```rust
/// Asserts that every query in one command decodes to its own request.
///
/// Case: a program saves two slots by asking for both in a single
/// command.
#[test]
fn each_query_in_one_command_decodes_separately() {
    assert_eq!(
        PaletteRequest::parse(&[b"4", b"0", b"?", b"1", b"?"]),
        vec![
            PaletteRequest::Query { index: 0 },
            PaletteRequest::Query { index: 1 }
        ]
    );
}
```

## TC-05 — 表の下端と上端の番号が通る

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"4", b"0", b"?", b"255", b"?"])` |
| Expect | `[Query { 0 }, Query { 255 }]` を返す **[D3]** |

色番号は ANSI の 0〜7、その bright 版 8〜15、そして 256 色表の残りに対応する。範囲の両端を
同時に確かめることで、`1..=255` のように 0 を落とす実装と、255 を上限外と見る実装の両方を
捕まえる。上限の外側は TC-06 が受け持つ。

```rust
/// Asserts that the first and last slots of the 256-colour table both
/// decode.
///
/// Case: a theme script recolors the terminal's black and its last
/// grayscale step in one pass.
#[test]
fn the_table_bounds_decode() {
    assert_eq!(
        PaletteRequest::parse(&[b"4", b"0", b"?", b"255", b"?"]),
        vec![
            PaletteRequest::Query { index: 0 },
            PaletteRequest::Query { index: 255 }
        ]
    );
}
```

## TC-06 — 表の外側の番号は切り詰めずに落とす

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"4", b"300", b"?", b"18446744073709551617", b"?"])` |
| Expect | 空の `Vec` を返す **[D3]** ／ 256 以上は `u8` へ切り詰めずに落とす **[P2]** |

300 も 20 桁の数も 256 色表の外側にあり、マニュアルはそれらに意味を与えていない。`u8` へ
キャストする実装だと 300 が 44 に化けて無関係なスロットを壊し、桁あふれする実装は 20 桁の数で
破綻する。どちらも「落とす」ことで避ける。なお 256〜260 はこのケースに入れていない — 付録の
「仕様の矛盾」のとおり、マニュアルはその 5 つを OSC 4 から設定できると定めているためである。

```rust
/// Asserts that a colour number outside the 256-colour table is dropped
/// rather than truncated to a byte or wrapped.
///
/// Case: a corrupted stream delivers colour numbers of 300 and twenty
/// digits.
#[test]
fn a_color_number_past_the_table_is_dropped() {
    assert!(
        PaletteRequest::parse(&[b"4", b"300", b"?", b"18446744073709551617", b"?"]).is_empty()
    );
}
```

## TC-07 — OSC 104 の番号は Reset になる

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"104", b"1"])` |
| Expect | `vec![PaletteRequest::Reset { index: 1 }]` を返す **[D7]** |

番号を伴う OSC 104 は、その 1 スロットだけを既定へ戻す。引数の有無で意味が変わる（D9）ので、
「引数あり」の側をここで押さえる。

```rust
/// Asserts that an `OSC 104` carrying a colour number decodes to a reset
/// of that slot.
///
/// Case: a program restores the one slot it recolored before it exits.
#[test]
fn an_osc_104_number_decodes_to_a_reset() {
    assert_eq!(
        PaletteRequest::parse(&[b"104", b"1"]),
        vec![PaletteRequest::Reset { index: 1 }]
    );
}
```

## TC-08 — OSC 104 の番号も何個でも並べられる

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"104", b"1", b"3"])` |
| Expect | `[Reset { 1 }, Reset { 3 }]` を順に返す **[D8]** ／ 番号は OSC 4 と同じ 256 色表を指す **[D10]** |

OSC 104 は番号を何個でも取り、その番号は OSC 4 と同じ表を指す。最初の 1 個だけを読む実装と、
番号を 1 個しか許さない実装を捕まえる。

```rust
/// Asserts that an `OSC 104` with several colour numbers decodes to a
/// reset of each slot, in order.
///
/// Case: a program restores the two slots it recolored before it exits.
#[test]
fn several_osc_104_numbers_reset_each_slot() {
    assert_eq!(
        PaletteRequest::parse(&[b"104", b"1", b"3"]),
        vec![
            PaletteRequest::Reset { index: 1 },
            PaletteRequest::Reset { index: 3 }
        ]
    );
}
```

## TC-09 — 引数の無い OSC 104 は表全体を戻す

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"104"])` |
| Expect | `vec![PaletteRequest::ResetAll]` を返す **[D9]** |

マニュアルは「引数が与えられなければ表全体がリセットされる」と定めている。これが D9 の条件を
満たす側で、満たさない側（番号つき）は TC-07 と TC-08 が同じ契約の裏を押さえているため、
別ケースには分けていない。番号の無い OSC 104 を何もしない命令として捨てる実装を捕まえる。

```rust
/// Asserts that an `OSC 104` with no colour number decodes to a reset of
/// every slot.
///
/// Case: `tput init` on an ncurses 6.6 entry sends `oc`, a bare
/// `OSC 104`, after a theme script recolored several slots.
#[test]
fn a_bare_osc_104_decodes_to_a_reset_of_every_slot() {
    assert_eq!(
        PaletteRequest::parse(&[b"104"]),
        vec![PaletteRequest::ResetAll]
    );
}
```

## TC-10 — 他の OS コマンドからは何も出ない

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse` に `["0","title"]`、`["5","0","?"]`、`["10","?"]`、`[]` |
| Expect | いずれも空の `Vec` を返す **[D1]** ／ リセットは 104 だけが担う **[D7]** |

この関数が答えるのは OSC 4 と OSC 104 の 2 つだけである。`OSC 5` はスペシャルカラー用の別命令、
`OSC 10` は dynamic color 用の別命令で、どちらもここでは扱わない。先頭の番号を数値として読んで
「4 で始まる」「104 を含む」といった緩い判定をする実装を捕まえる。空の params は、番号の無い
OS コマンドが来たときの下端。

```rust
/// Asserts that every other operating system command decodes to no
/// palette request.
///
/// Case: a shell sets its title while a script sets xterm's special and
/// dynamic colors, which this terminal ignores or handles elsewhere.
#[test]
fn other_commands_decode_to_no_palette_request() {
    assert!(PaletteRequest::parse(&[b"0", b"title"]).is_empty());
    assert!(PaletteRequest::parse(&[b"5", b"0", b"?"]).is_empty());
    assert!(PaletteRequest::parse(&[b"10", b"?"]).is_empty());
    assert!(PaletteRequest::parse(&[]).is_empty());
}
```

## TC-11 — 相方の無い最後の番号は無視する

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"4", b"1", b"?", b"2"])` |
| Expect | `vec![PaletteRequest::Query { index: 1 }]` を返す **[D2]** ／ 相方の無い番号は無視する **[P3]** |

マニュアルが許すのは「組」であり、番号ひとつは組ではない。パーサのパラメータ上限で 1 本が
途中で切れると必ずこの形で終わるので、最後の番号を spec 欠落の Set として扱う実装や、
パラメータ数が偶数だからと 1 本まるごと捨てる実装を捕まえる。

```rust
/// Asserts that an unpaired trailing colour number is ignored while the
/// pairs before it still decode.
///
/// Case: the parser's parameter cap cuts a long `OSC 4` off between a
/// colour number and its spec.
#[test]
fn an_unpaired_trailing_number_is_ignored() {
    assert_eq!(
        PaletteRequest::parse(&[b"4", b"1", b"?", b"2"]),
        vec![PaletteRequest::Query { index: 1 }]
    );
}
```

## TC-A1 — 読めない spec はその組だけ落ちる

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"4", b"1", b"red", b"2", b"rgb:00/00/ff"])` |
| Expect | `vec![PaletteRequest::Set { index: 2, color: rgb(0x00, 0x00, 0xff) }]` を返す **[P1]** |

マニュアルは組が読めなかったときの振る舞いを定めていない（xterm の実装は最初の誤りで打ち切る）。
このケースはリポジトリの doc comment が宣言する「その組だけ落として続行する」方針だけを根拠に
しているので `TC-A` 番号を振ってある。色名 `red` を選んだのは、この端末が色名を実装しないため
確実に読めない spec になるからで、色名を拒否する判断そのものは `Rgb::from_color_spec` 側の
仕様の矛盾として記録済みである。

```rust
/// Asserts that a pair whose spec cannot be read is dropped on its own
/// while the pairs after it still decode.
///
/// Case: a script written for xterm recolors slot 1 by the name `red`,
/// which this terminal does not know, before recoloring slot 2.
#[test]
fn an_unreadable_spec_drops_only_its_own_pair() {
    assert_eq!(
        PaletteRequest::parse(&[b"4", b"1", b"red", b"2", b"rgb:00/00/ff"]),
        vec![PaletteRequest::Set {
            index: 2,
            color: rgb(0x00, 0x00, 0xff)
        }]
    );
}
```

## TC-A2 — 十進数でない番号は落とす

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `PaletteRequest::parse(&[b"4", b"", b"?", b"x", b"?", b"+1", b"?"])` |
| Expect | 空の `Vec` を返す **[P1]** |

番号が読めないときの扱いもマニュアルにはなく、doc comment の「読めない番号はそれだけ落とす」に
依っている。`+1` を入れてあるのは、`str::parse` に丸投げした実装が符号を受け付けてスロット 1 へ
化けるためで、空文字列と `x` はそれぞれ長さ 0 と非数字の下端。

```rust
/// Asserts that a colour number that is not a plain decimal is dropped.
///
/// Case: a buggy script formats colour numbers with a sign or a letter,
/// or leaves one empty.
#[test]
fn a_malformed_color_number_is_dropped() {
    assert!(PaletteRequest::parse(&[b"4", b"", b"?", b"x", b"?", b"+1", b"?"]).is_empty());
}
```

---

## 付録

### 契約表

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| D1 | OSC 4 | unconditional | Ps = 4 ; c ; spec, Change Color Number c to the color specified by spec. | xterm-ctlseqs.pdf p.38, L2059-2060 |
| D2 | OSC 4 | multi-function | Any number of c/spec pairs may be given. | xterm-ctlseqs.pdf p.38, L2062-2063 |
| D3 | OSC 4 | bounded value | The color numbers correspond to the ANSI colors 0-7, their bright versions 8-15, and if supported, the remainder of the 88-color or 256-color table. | xterm-ctlseqs.pdf p.38, L2063-2066 |
| D4 | OSC 4 の `?` | conditional | If a "?" is given rather than a name or RGB specification, xterm replies with a control sequence of the same form which can be used to set the corresponding color. | xterm-ctlseqs.pdf p.38, L2068-2070 |
| D5 | OSC 4 の `?` | unconditional | Because more than one pair of color number and specification can be given in one control sequence, xterm can make more than one reply. | xterm-ctlseqs.pdf p.38, L2070-2072 |
| D6 | OSC 5 / Special Colors | bounded value | The special colors can also be set by adding the maximum number of colors (e.g., 88 or 256) to these codes in an OSC 4 control | xterm-ctlseqs.pdf p.39, L2077-2079 |
| D7 | OSC 104 | unconditional | Ps = 1 0 4 ; c, Reset Color Number c. It is reset to the color specified by the corresponding X resource. | xterm-ctlseqs.pdf p.42, L2243-2244 |
| D8 | OSC 104 | multi-function | Any number of c parameters may be given. | xterm-ctlseqs.pdf p.42, L2244-2245 |
| D9 | OSC 104 | conditional | If no parameters are given, the entire table will be reset. | xterm-ctlseqs.pdf p.42, L2248 |
| D10 | OSC 104 | bounded value | These parameters correspond to the ANSI colors 0-7, their bright versions 8-15, and if supported, the remainder of the 88-color or 256-color table. | xterm-ctlseqs.pdf p.42, L2245-2247 |

kind-2 台帳:

```
P1 — crates/orzma_vt/src/interpreter/osc.rs:83-84, PaletteRequest::parse
     "A pair or number that cannot be read is dropped on its own and"
     "the rest still decode, where xterm stops at the first error."
     justifies: TC-A1、TC-A2

P2 — crates/orzma_vt/src/interpreter/osc.rs:85-86, PaletteRequest::parse
     "colour number of 256 or more is dropped rather than truncated to"
     "a byte"
     justifies: TC-06 の「切り詰めない」行

P3 — crates/orzma_vt/src/interpreter/osc.rs:87-89, PaletteRequest::parse
     "An unpaired trailing"
     "number, which is how a command cut short by the parser's"
     "parameter cap ends, is ignored."
     justifies: TC-11 の無視する行
```

### 探索した語

| 語 | Level | 結果 |
| - | - | - |
| `OSC 4` / `Ps = 4 ; c ; spec` | 1 | xterm-ctlseqs.pdf L2059。D1〜D5 に到達 |
| `Change Color Number` | 1 | 同 L2059。D1 に到達 |
| `c/spec pairs` | 1 | 同 L2063、L2077。D2 に到達 |
| `OSC 104` / `Ps = 1 0 4` | 1 | 同 L2243。D7〜D10 に到達。なお `Ps = 1 0 4 x` は DECSET の 1040〜1049 と `OSC 104`（背景色）にも当たるため、1d の規則で定義以外の hit を捨てた（捨てた hit は 20 件） |
| `Reset Color Number` | 1 | 同 L2243。D7 に到達 |
| `entire table will be reset` | 1 | 同 L2248。D9 に到達 |
| `more than one reply` | 1 | 同 L2072。D5 に到達 |
| `special colors can also be set` | 1 | 同 L2078。D6 に到達（仕様の矛盾として記録） |
| `XParseColor` | 1, 4 | 同 L2063、L2077、L2106。spec の文法は `Rgb::from_color_spec` 側の担当なので、このメソッドの契約には使っていない |
| `operating system command` | 3 | ファイルの `//!` 由来。OSC の総論には届くが、個別の振る舞いには到達しない |
| `palette` / `indexed` | 2, 4 | enum の doc と signature 由来。マニュアルは "color table" / "color numbers" と呼ぶため、D3・D10 経由でのみ到達 |
| `ECMA-48` の OSC 定義 | 4 | ECMA-48.pdf は OSC を「文字列を導入する制御機能」としてしか定義しておらず、色番号の意味には到達しない |
| vt510 / vt220 の `OSC` | 4 | どちらも OSC 4 / 104 を扱っていない（DEC 端末に色表が無い）。振る舞いの記述には到達しなかった |

### 仕様にないもの

- **`["104", ""]`（空の引数 1 つ）が表全体のリセットになるか**: D9 は「引数が与えられなければ」と
  書いており、空文字列の引数が「与えられていない」に当たるかは定めていない。タグを付けられない
  のでケースにしていない。実装計画はこれを ResetAll として扱う方針のテストを別に持っている。
- **先頭のゼロ（`007` が 7 として読まれること）**: OSC の文字列引数の数値表記について、どの
  マニュアルも規定していない。同じく計画側のテストが方針として持つ。
- **読めない組・番号を落として続行すること**: マニュアルは誤りの扱いを定めていない（TC-A1、
  TC-A2 が doc comment だけを根拠にしているのはこのため）。

### 仕様の矛盾

1 件あり、ケースは出していない。

- **D6（検証済み、xterm-ctlseqs.pdf p.39 L2077-2079）**: "The special colors can also be set by
  adding the maximum number of colors (e.g., 88 or 256) to these codes in an OSC 4 control" ——
  つまり `OSC 4 ; 256`〜`260` はスペシャルカラー（colorBD / UL / BL / RV / IT）の設定として
  有効である。
- **リポジトリの doc comment（crates/orzma_vt/src/interpreter/osc.rs:85-87）**: "A colour number of
  256 or more is dropped rather than truncated to a byte: xterm reaches its special colors at 256
  through 260, and a truncated 256 would overwrite slot 0."

マニュアルは 256〜260 を有効な設定先と認め、doc comment は落とすと宣言している。リポジトリの
記述が検証済みのマニュアルを覆すことはできないので、256〜260 を対象にしたケースはこのリストに
無い（TC-06 は 261 以上、マニュアルが意味を与えていない範囲だけを扱う）。スペシャルカラーを
実装しない判断は設計（spec の決定 3）で下されており、仕様からの意図的な逸脱として扱うのが筋で
ある。

### API 改定

無し。`parse(params: &[&[u8]]) -> Vec<Self>` は上の 13 ケースすべてを表現できる。

### 出典なしの改善提案

無し。

### docs/todo との衝突

無し。`docs/todo/` を `PaletteRequest` / `OSC 4` / `OSC 104` / `palette` で検索して当たったのは
2 件で、どちらも API の食い違いではない。

- `vt-conformance-scope.md:68` は OSC 4 を未実装（`OSC∅`）として挙げている。これは実装前の
  状態を記した表で、実装計画の Task 6 が更新する。
- `tdd-color-from_color_spec.md` は同じ一連の作業で書いた `Rgb::from_color_spec` のケース一覧で、
  spec 文字列の文法だけを扱う。このメソッドとは担当が分かれており、重なりは無い。
