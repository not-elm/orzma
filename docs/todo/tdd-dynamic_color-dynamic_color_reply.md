# Test cases: dynamic_color_reply

`crates/orzma_vt/src/interpreter/osc/dynamic_color.rs` の
`dynamic_color_reply` が負うテストケース。仕様は
`docs/references/xterm-ctlseqs.pdf` と、それが `XParseColor` 経由で委ねる
`docs/references/xlib.pdf` から導出した。citation は 6/6 検証済み。Phase 4 で
著者が一覧を承認した。

signature の変更は提案していない。
`dynamic_color_reply(target: DynamicColor, color: Rgb, terminator: OscTerminator) -> Vec<u8>`
は `palette_reply` と同型で、4 件すべてを表現できる。

**このドキュメントの Rust はビルドしていない。** 現状の木に対して
**コンパイルできる**（`dynamic_color_reply` は空の `Vec` を返すスタブとして既に
定義されている）が、転記して `cargo test` を通すのは読み手の仕事で、この一覧は
仕様由来の自己検証にとどまる。citation を 1〜2 本自分で引き直してから残りを
信用してほしい。

## テストケース一覧

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-R1 | `a_foreground_reply_names_its_own_number` | C4, C7, D1, D2, D3 | High |
| TC-R2 | `a_background_reply_names_its_own_number` | C5, C7 | High |
| TC-R3 | `an_st_closed_query_is_answered_with_the_seven_bit_string_terminator` | D1 | High |
| TC-R4 | `a_reply_reads_back_to_the_color_it_reports` | C7 | High |

Source タグ — **C4**: xterm p.40 L2133（OSC 10 = 前景）／**C5**: xterm p.40
L2134（OSC 11 = 背景）／**C7**: xterm p.40 L2126（`?` の応答は同じ形で、その色を
設定するのに使える）／**D1**: xterm p.38 L2042（問い合わせと同じ終端を使う）／
**D2**: xlib p.90 L5086（`hhhh` は 16 ビットに尺度化した値）／**D3**: xlib p.89
L5075（`rgb:<red>/<green>/<blue>` の構文）。全エントリは付録の契約表にある。

4 件すべてが仕様由来で、`TC-A` は無い。

テストコードは `crates/orzma_vt/src/interpreter/osc/dynamic_color.rs` の
`#[cfg(test)] mod tests` へ、`DynamicColorRequest::parse` の 13 件の後に追加する。
同モジュールの `fn rgb(r: u8, g: u8, b: u8) -> Rgb` ヘルパをそのまま使う。
`dynamic_color_reply` は純関数なので、どのケースにも setup は要らない。

## TC-R1 — 前景の応答は自分の番号 `10` を名乗る

| | |
| - | - |
| Setup | なし |
| Act | `dynamic_color_reply(DynamicColor::Foreground, rgb(0xcd, 0x00, 0x00), OscTerminator::Bel)` |
| Expect | 番号 `10` を名乗る **[C4][C7]** ／ spec が `rgb:<red>/<green>/<blue>` の形をとる **[D3]** ／ 各チャンネルが 8 ビット値を 2 回並べた 4 桁になる **[D2]** ／ BEL で閉じる **[D1]** |

4 桁にするのは D2 が `hhhh` を「16 ビットに尺度化した値」と定めるからで、
`rgb:cd` を Xlib の規則で 16 ビットへ広げた値（`0xcdcd`）と一致する。8 ビットで
保持している値を欠損なく往復させられる唯一の綴りがこれ。色に stock の xterm
red を選んだのは、`palette_reply` の同型テストと同じ値で、丸め規則の差が
結果に出ないことを確かめやすいため。

**小文字であることはどのマニュアルも要求していない** — D3 は "case
insignificant" と述べるだけ。`palette_reply` に揃えた選択で、付録の
「仕様にないもの」に記録してある。

```rust
/// Asserts that a foreground reply names its own colour number and
/// writes each channel as the four hex digits an eight-bit value scales
/// to, closing with the terminator the query used.
///
/// Case: a program asks for the terminal's text colour with a
/// BEL-closed `OSC 10 ; ?`.
#[test]
fn a_foreground_reply_names_its_own_number() {
    assert_eq!(
        dynamic_color_reply(
            DynamicColor::Foreground,
            rgb(0xcd, 0x00, 0x00),
            OscTerminator::Bel
        ),
        b"\x1b]10;rgb:cdcd/0000/0000\x07"
    );
}
```

## TC-R2 — 背景の応答は自分の番号 `11` を名乗る

| | |
| - | - |
| Setup | なし |
| Act | `dynamic_color_reply(DynamicColor::Background, rgb(0x20, 0x20, 0x20), OscTerminator::Bel)` |
| Expect | 番号 `11` を名乗る **[C5][C7]** |

C7 の "the corresponding dynamic color" が効くのがここ。連鎖問い合わせ
`OSC 10 ; ? ; ?` には 2 本返すことになり、両方が `10` を名乗ると、受け取った
プログラムは背景を前景として復元する。番号を定数で書いた実装は前景の
ケースだけ通って、このケースで落ちる。

```rust
/// Asserts that a background reply names its own colour number rather
/// than the number of the color the query started at.
///
/// Case: nvim asks for the background at startup, and a theme script
/// asks for both colours in one chained query.
#[test]
fn a_background_reply_names_its_own_number() {
    assert_eq!(
        dynamic_color_reply(
            DynamicColor::Background,
            rgb(0x20, 0x20, 0x20),
            OscTerminator::Bel
        ),
        b"\x1b]11;rgb:2020/2020/2020\x07"
    );
}
```

## TC-R3 — ST で閉じた問い合わせには 7 ビットの ST で答える

| | |
| - | - |
| Setup | なし |
| Act | `dynamic_color_reply(DynamicColor::Foreground, rgb(0xab, 0x12, 0xef), OscTerminator::St)` |
| Expect | `ESC \` で閉じる **[D1]** |

D1 が "uses the same terminator used in a query" と定める。8 ビットの `0x9C` で
届いた問い合わせにも 7 ビットの `ESC \` で返すのは `OscTerminator` が既に
畳んでいる契約で、そちらは `osc.rs` のテストが押さえている。ここで見るのは
終端が応答に反映されることだけ。

```rust
/// Asserts that a query closed with the string terminator is answered
/// with its seven-bit form.
///
/// Case: a terminfo-driven program closes its query with `ESC \`
/// instead of BEL.
#[test]
fn an_st_closed_query_is_answered_with_the_seven_bit_string_terminator() {
    assert_eq!(
        dynamic_color_reply(
            DynamicColor::Foreground,
            rgb(0xab, 0x12, 0xef),
            OscTerminator::St
        ),
        b"\x1b]10;rgb:abab/1212/efef\x1b\\"
    );
}
```

## TC-R4 — 応答は報告した色に読み戻せる

| | |
| - | - |
| Setup | なし |
| Act | 応答から `\x1b]` と終端を剥がし、`;` で分割して `DynamicColorRequest::parse` へ戻す。前景・背景の両方と、チャンネル `0x00, 0x01, 0x7f, 0x80, 0xcd, 0xfe, 0xff` の組で回す |
| Expect | 元と同じ `target` と `color` を持つ `Set` が 1 件返る **[C7]** |

C7 の "which can be used to set the corresponding dynamic color" を文字どおり
試すケース。`osc/palette.rs` の同型テストは spec を `Rgb::from_color_spec` に
だけ戻すが、こちらはデコーダ全体を通すので、番号の取り違えと桁の欠損を同時に
捕まえる。チャンネルには両端（`0x00` と `0xff`）と、16 進 1 桁では表せない値
（`0x01`、`0x7f`、`0xfe`）を含めてある。

```rust
/// Asserts that a reply decodes back to the color and the target it
/// reports, so replaying it sets the same color again.
///
/// Case: a program saves a dynamic colour with a query and later
/// restores it by sending the reply back to the terminal.
#[test]
fn a_reply_reads_back_to_the_color_it_reports() {
    for target in [DynamicColor::Foreground, DynamicColor::Background] {
        for channel in [0x00, 0x01, 0x7f, 0x80, 0xcd, 0xfe, 0xff] {
            let color = rgb(channel, channel, channel);
            let reply = dynamic_color_reply(target, color, OscTerminator::Bel);
            let body = reply
                .strip_prefix(b"\x1b]".as_slice())
                .and_then(|rest| rest.strip_suffix(b"\x07".as_slice()))
                .expect("the reply is framed as OSC Ps ; spec BEL");
            let params: Vec<&[u8]> = body.split(|byte| *byte == b';').collect();
            assert_eq!(
                DynamicColorRequest::parse(&params),
                vec![DynamicColorRequest::Set { target, color }]
            );
        }
    }
}
```

## 付録

### 契約表

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C4 | OSC 10 | unconditional | Change VT100 text foreground color to Pt. | xterm-ctlseqs.pdf p.40, L2133 |
| C5 | OSC 11 | unconditional | Change VT100 text background color to Pt. | xterm-ctlseqs.pdf p.40, L2134 |
| C7 | OSC 10–19 | conditional | If a "?" is given rather than a name or RGB specification, xterm replies with a control sequence of the same form which can be used to set the corresponding dynamic color. | xterm-ctlseqs.pdf p.40, L2126-2131 |
| D1 | OSC 終端 | unconditional | XTerm accepts either BEL or ST for terminating OSC sequences, and when returning information, uses the same terminator used in a query. | xterm-ctlseqs.pdf p.38, L2042-2044 |
| D2 | RGB Device String | unconditional | h indicates the value scaled in 4 bits, hh the value scaled in 8 bits, hhh the value scaled in 12 bits, and hhhh the value scaled in 16 bits, respectively. | xlib.pdf p.90, L5086-5087 |
| D3 | RGB Device String | unconditional | `rgb:<red>/<green>/<blue>` ／ `<red>, <green>, <blue> := h \| hh \| hhh \| hhhh` | xlib.pdf p.89, L5075-5077 |

D3 は最初 "An RGB Device specification is identified by the prefix rgb: …" と
記録して REJECTED（3/20 matched）になった。**`pdftotext` の出力が合字を残して
いる**のが原因で、`speciﬁcation` の `ﬁ` は U+FB01、`preﬁx` も同様。検証器の
トークナイザ `[A-Za-z0-9]+` はこれを `speci` と `cation` に割るので、普通に
綴った引用は決して一致しない。合字を含まない構文の行（L5075-5077）へ引用を
狭めて再検証した。xlib.pdf を引く後続のランは同じ罠を踏むので、`ﬁ` `ﬀ` `ﬂ` を
含む語を引用に入れないこと。

### 探索した語

| Level | Source | 語 | 結果 |
| - | - | - | - |
| 1 | `dynamic_color_reply` の `///` | OSC 10, OSC 11, reply | C7 に到達 |
| 3 | ファイルの `//!` | dynamic foreground, background, queries | C4, C5 に到達 |
| 4 | signature の型名（`OscTerminator`） | BEL, ST, terminating OSC | D1 に到達 |
| 4 | signature の型名（`Rgb`）と C3 の委譲先 | RGB Device String, rgb:, XParseColor | D2, D3 に到達 |

DEC 系 3 冊は前回のラン（`tdd-dynamic_color-parse.md`）で dynamic colors を
扱っていないことを確認済みなので、再探索していない。

### 仕様にないもの

- **16 進を小文字で書くこと。** D3 は `h := single hexadecimal digits (case
  insignificant)` と述べるだけで、応答の綴りに大小の指定は無い。`palette_reply`
  が小文字で書いているのに揃えた。TC-R1 と TC-R3 の期待値がこれを暗黙に固定
  するので、変えるときはこの 2 件が落ちる。
- **8 ビットで保持していること自体。** マニュアルは 16 ビットを前提にしており、
  orzma が `Rgb` を 8 ビットで持つのはこのクレートの決定（`docs/memo/palette.md`
  に理由がある）。応答が 4 桁になるのはその決定の帰結で、D2 はその綴りが 16
  ビット値として正しく読まれることを保証するにとどまる。

### 仕様の矛盾

無し。

### API 改定

無し。

### 出典なしの改善提案

無し。

### docs/todo との衝突

無し。`vt-conformance-scope.md:242` との衝突は `tdd-dynamic_color-parse.md` に
記録済みで、こちらのメソッドには及ばない。
