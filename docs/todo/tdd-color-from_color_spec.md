# Test cases: Rgb::from_color_spec

`crates/orzma_vt/src/device/color.rs:124` の `Rgb::from_color_spec` が負うテストケースを、
`docs/references/xlib.pdf`（Color Strings / RGB Device String Specification）と
`docs/references/xterm-ctlseqs.pdf`（OSC 4 の spec パラメータ）から洗い出した。
citations verified: 11/11（マニュアル 8 件、リポジトリの doc comment 3 件）。
Phase 4 の結果: 13 件すべて承認（2026-09-12）。signature の変更提案は無し。

**このドキュメントの Rust は現状の木に対してコンパイルは通るが、通らない（fail する）。**
`from_color_spec` の本体はまだ `todo!()` のスタブなので、どのテストも panic で失敗する。
それが実装計画の RED ステップであり、本体を書いた時点で GREEN になる。
**ビルド・実行は一度もしていない**。貼って走らせるものではなく、書き写すための提案である。

このリストを点検したのは私だけで、レビューは受けていない。仕様由来であることと自己検証済みで
あることだけが根拠なので、信用する前に citation を 1〜2 件抜き取りで確かめてほしい。

## テストケース一覧

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `a_one_digit_rgb_component_is_scaled_across_the_channel` | C3, N | High |
| TC-02 | `a_two_digit_rgb_string_reads_its_components_directly` | C3, N | High |
| TC-03 | `wide_rgb_components_narrow_to_their_high_byte` | C3, N | High |
| TC-04 | `components_of_different_widths_mix_in_one_string` | C4, C3, N | High |
| TC-05 | `a_sharp_string_places_each_component_in_the_high_bits` | C5, C6, N | High |
| TC-06 | `the_manual_sharp_example_reads_as_its_16_bit_equivalent` | C6, N | High |
| TC-07 | `the_prefix_and_the_digits_ignore_case` | C1, N | High |
| TC-08 | `a_sharp_string_of_another_length_is_refused` | C5 | High |
| TC-09 | `an_rgb_string_without_exactly_three_components_is_refused` | C2, S | High |
| TC-10 | `an_empty_or_over_long_rgb_component_is_refused` | C2, C3 | High |
| TC-11 | `a_non_hex_digit_is_refused` | C2 | High |
| TC-12 | `other_color_spaces_are_refused` | C2 | High |
| TC-A1 | `surrounding_whitespace_is_refused_rather_than_trimmed` | W | Medium |

Source タグ — **C1**: xlib.pdf p.89 L5027-5029（色文字列は大小を区別しない）／**C2**: xlib.pdf p.89
L5075-5077（`rgb:` の文法と成分の桁数）／**C3**: xlib.pdf p.90 L5086-5087（桁数ごとのスケール）／
**C4**: xlib.pdf p.90 L5089-5091（幅の混在）／**C5**: xlib.pdf p.90 L5100-5103（`#` の 4 つの幅）／
**C6**: xlib.pdf p.90 L5106-5108（`#` は上位ビット、`#3a7` = `#3000a0007000`）／**C7**: xterm-ctlseqs.pdf
p.38 L2062-2063（OSC 4 の spec は XParseColor）／**N**: color.rs:113（16 ビット値を上位バイトへ狭める）／
**W**: color.rs:120-121（前後の空白を削らない）／**S**: color.rs:119-120（3 成分より後ろがあれば拒否）。
全文は付録の契約表にある。

TC-01 から TC-12 までは仕様が明示している振る舞いとその境界で、マニュアルの文が根拠になっている。
TC-A1 だけはリポジトリの doc comment しか根拠が無い（`TC-A` の番号はそれを表す）。なお 8 ビットへ
狭める規則（タグ **N**）はどのマニュアルにも無く、orzma 自身の契約なので、ほぼ全ケースの `Expect:`
がこのタグを併記している。優先度は High → Medium の順に並べてあるので、High まで読めば仕様が
明示している範囲は尽きる。

テストコードは `crates/orzma_vt/src/device/color.rs` の `mod tests`（line 285、サブモジュール無しの
平坦な構成）へ追加する。既存のヘルパは無く、`Rgb { r, g, b }` を組み立てるだけなので、下の
`fn rgb(r: u8, g: u8, b: u8) -> Rgb` を同じモジュールに置いて使う。

```rust
    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }
```

---

## TC-01 — 1 桁の `rgb:` 成分は channel 全体へスケールする

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec(b"rgb:f/8/0")` |
| Expect | `Some(rgb(0xff, 0x88, 0x00))` を返す **[C3]** ／ 8 ビット値は 16 ビット値の上位バイト **[N]** |

1 桁は「4 ビットに収めた値」なので、0xf は 0xffff、0x8 は 0x8888 へ広がる。上位バイトを取ると
0xff と 0x88 になる。`#` 形式と取り違えて上位ビットに置く実装（0xf → 0xf0）を捕まえるのがこの
ケースの役目で、TC-05 と対になっている。

```rust
/// Asserts that a one-digit `rgb:` component is scaled across the
/// channel rather than placed in its high bits.
///
/// Case: a theme writes its colours in the shorthand `rgb:f/8/0`.
#[test]
fn a_one_digit_rgb_component_is_scaled_across_the_channel() {
    assert_eq!(
        Rgb::from_color_spec(b"rgb:f/8/0"),
        Some(rgb(0xff, 0x88, 0x00))
    );
}
```

## TC-02 — 2 桁の `rgb:` 成分はそのままのバイトになる

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec(b"rgb:ff/80/0a")` |
| Expect | `Some(rgb(0xff, 0x80, 0x0a))` を返す **[C3]** ／ 8 ビット値は 16 ビット値の上位バイト **[N]** |

2 桁は「8 ビットに収めた値」で、16 ビットへ広げると同じバイトが 2 回並ぶ（0x80 → 0x8080）。
上位バイトは元の値に戻るので、terminfo の `initc` が送るこの形は往復しても変わらない。

```rust
/// Asserts that an `rgb:` string with two-digit components narrows to
/// exactly those bytes.
///
/// Case: terminfo's `initc` recolors a slot with `rgb:ff/80/0a`.
#[test]
fn a_two_digit_rgb_string_reads_its_components_directly() {
    assert_eq!(
        Rgb::from_color_spec(b"rgb:ff/80/0a"),
        Some(rgb(0xff, 0x80, 0x0a))
    );
}
```

## TC-03 — 3 桁・4 桁の成分は上位バイトへ丸める

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec(b"rgb:fff/800/000")` と `Rgb::from_color_spec(b"rgb:ffff/8080/0000")` |
| Expect | どちらも `Some(rgb(0xff, 0x80, 0x00))` を返す **[C3]** ／ 8 ビット値は 16 ビット値の上位バイト **[N]** |

3 桁は 12 ビット、4 桁は 16 ビットに収めた値で、どちらも 16 ビットへそろえてから上位バイトを取る。
0x800 は 12 ビットのスケールで 0x8007 になり、上位バイトは 0x80。4 桁はスケールが恒等なので
0x8080 → 0x80。このケースが押さえるのは、4 桁までが受理されること（5 桁は TC-10 が拒否する）と、
残るのが下位バイトではなく上位バイトであることの 2 点である。なお 3 桁のスケールと「4 ビット
左シフト」はここで挙げた値では同じ答えになるので、スケールと上位ビット詰めを分けて捕まえる役は
TC-01 と TC-05 が担っている。

```rust
/// Asserts that three- and four-digit `rgb:` components narrow to the
/// high byte of their 16-bit value.
///
/// Case: a colour picker exports its colours at twelve and sixteen bits
/// per channel.
#[test]
fn wide_rgb_components_narrow_to_their_high_byte() {
    assert_eq!(
        Rgb::from_color_spec(b"rgb:fff/800/000"),
        Some(rgb(0xff, 0x80, 0x00))
    );
    assert_eq!(
        Rgb::from_color_spec(b"rgb:ffff/8080/0000"),
        Some(rgb(0xff, 0x80, 0x00))
    );
}
```

## TC-04 — 幅の違う成分が 1 つの文字列に混在できる

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec(b"rgb:ff/a5/0")` と `Rgb::from_color_spec(b"rgb:ccc/32/0")` |
| Expect | `Some(rgb(0xff, 0xa5, 0x00))` と `Some(rgb(0xcc, 0x32, 0x00))` を返す **[C4]** ／ 各成分はその成分自身の桁数でスケールされる **[C3]** ／ 8 ビット値は 16 ビット値の上位バイト **[N]** |

マニュアルが挙げている 2 例そのもの。成分ごとに桁数が違ってよいので、スケールの分母は文字列
全体ではなく成分ごとに決まる。最初の成分の桁数を残り 2 つにも使い回す実装を捕まえる。

```rust
/// Asserts that components of different widths mix in one `rgb:`
/// string, each scaled by its own width.
///
/// Case: a hand-written theme spells its colours as `rgb:ff/a5/0` and
/// `rgb:ccc/32/0`.
#[test]
fn components_of_different_widths_mix_in_one_string() {
    assert_eq!(
        Rgb::from_color_spec(b"rgb:ff/a5/0"),
        Some(rgb(0xff, 0xa5, 0x00))
    );
    assert_eq!(
        Rgb::from_color_spec(b"rgb:ccc/32/0"),
        Some(rgb(0xcc, 0x32, 0x00))
    );
}
```

## TC-05 — `#` 形式は 4 つの幅すべてで上位ビットに置く

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec` に `b"#f80"`、`b"#ff8000"`、`b"#fff800000"`、`b"#ffff80000000"` |
| Expect | `#f80` は `Some(rgb(0xf0, 0x80, 0x00))`、残りは `Some(rgb(0xff, 0x80, 0x00))` **[C5]** ／ 16 ビットに満たない桁は上位ビットとして置かれ、スケールされない **[C6]** ／ 8 ビット値は 16 ビット値の上位バイト **[N]** |

`#` は 3 / 6 / 9 / 12 桁の 4 つだけが定義されており、いずれも桁を上位ビットへ詰める。だから
`#f80` の赤は 0xf0 であって、`rgb:f/8/0` の 0xff とは別の色になる。両形式を同じスケール規則で
処理する実装を捕まえるのがこのケースで、TC-01 と対になっている。

```rust
/// Asserts that a `#` string places each component in the high bits, at
/// every width the grammar allows.
///
/// Case: an older theme writes its orange at each width the `#` form
/// allows.
#[test]
fn a_sharp_string_places_each_component_in_the_high_bits() {
    assert_eq!(Rgb::from_color_spec(b"#f80"), Some(rgb(0xf0, 0x80, 0x00)));
    assert_eq!(
        Rgb::from_color_spec(b"#ff8000"),
        Some(rgb(0xff, 0x80, 0x00))
    );
    assert_eq!(
        Rgb::from_color_spec(b"#fff800000"),
        Some(rgb(0xff, 0x80, 0x00))
    );
    assert_eq!(
        Rgb::from_color_spec(b"#ffff80000000"),
        Some(rgb(0xff, 0x80, 0x00))
    );
}
```

## TC-06 — マニュアルの例 `#3a7` は `#3000a0007000` と同じ色になる

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec(b"#3a7")` |
| Expect | `Some(rgb(0x30, 0xa0, 0x70))` を返す **[C6]** ／ 8 ビット値は 16 ビット値の上位バイト **[N]** |

マニュアルが `#3a7` = `#3000a0007000` と書いている、その例そのものを 8 ビットに狭めた形。
16 ビットの中間値は 0x3000 / 0xa000 / 0x7000 なので、上位バイトは 0x30 / 0xa0 / 0x70 になる。
桁を下位に置く実装（0x3 → 0x03）も、スケールする実装（0x3 → 0x33）も、このケースで落ちる。

```rust
/// Asserts that the `#` example the Xlib manual gives reads as the
/// 16-bit value it names, narrowed to a byte.
///
/// Case: a theme carried over from an X resource file writes its colour
/// as `#3a7`.
#[test]
fn the_manual_sharp_example_reads_as_its_16_bit_equivalent() {
    assert_eq!(Rgb::from_color_spec(b"#3a7"), Some(rgb(0x30, 0xa0, 0x70)));
}
```

## TC-07 — prefix も 16 進の桁も大小を区別しない

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec(b"RGB:FF/A5/0")` と `Rgb::from_color_spec(b"#FFA500")` |
| Expect | どちらも `Some(rgb(0xff, 0xa5, 0x00))` を返す **[C1]** ／ 8 ビット値は 16 ビット値の上位バイト **[N]** |

色文字列は大小を区別しない、とマニュアルが明言している。terminfo の `initc` は 16 進を大文字で
送るので、桁側の大小無視は実務上も必要になる。prefix をバイト列の完全一致で比べる実装と、
`to_digit` を使わず `b'a'..=b'f'` だけを見る実装の両方を捕まえる。

```rust
/// Asserts that the prefix and the hex digits are read without regard
/// to case.
///
/// Case: one program writes `RGB:FF/A5/0` and another `#FFA500`.
#[test]
fn the_prefix_and_the_digits_ignore_case() {
    assert_eq!(
        Rgb::from_color_spec(b"RGB:FF/A5/0"),
        Some(rgb(0xff, 0xa5, 0x00))
    );
    assert_eq!(
        Rgb::from_color_spec(b"#FFA500"),
        Some(rgb(0xff, 0xa5, 0x00))
    );
}
```

## TC-08 — `#` の桁数が 3 / 6 / 9 / 12 以外なら拒否する

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec` に `b"#"`、`b"#12"`、`b"#1234"`、`b"#1234567890abc"` |
| Expect | すべて `None` を返す **[C5]** |

マニュアルが挙げる `#` の形は 4 つだけで、その 4 つは「3 で割り切れ、商が 1〜4 桁」に一致する。
3 で割り切れない 4 桁（`#1234`）と、商が 5 桁になる 13 桁（`#1234567890abc`）が境界の外側で、
桁数を検査せず 3 等分する実装（`#1234` を 1 桁ずつ読んで末尾を捨てる）を捕まえる。空の `#` は
長さ 0 の成分になるので、0 桁側の境界も兼ねる。

```rust
/// Asserts that a `#` string whose digits do not split into three equal
/// components of one to four digits is refused.
///
/// Case: a typo leaves a theme colour with four or thirteen hex digits,
/// or none at all.
#[test]
fn a_sharp_string_of_another_length_is_refused() {
    for spec in [b"#".as_slice(), b"#12", b"#1234", b"#1234567890abc"] {
        assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
    }
}
```

## TC-09 — `rgb:` の成分がちょうど 3 つでなければ拒否する

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec` に `b"rgb:"`、`b"rgb:ff/ff"`、`b"rgb:ff/ff/ff/ff"` |
| Expect | すべて `None` を返す **[C2]** ／ 3 成分より後ろに何かあれば拒否する **[S]** |

文法は `rgb:<red>/<green>/<blue>` の 3 成分ちょうどで、それ未満も超過も文法に合わない。libX11 の
パーサは 4 つ目以降を読み捨てるので `rgb:1/2/3/4` を受け付けるが、orzma は doc comment のとおり
意図的に厳しくしており、その差がこのケースに出る。

```rust
/// Asserts that an `rgb:` string with more or fewer than three
/// components is refused, a trailing fourth included, rather than
/// having the tail ignored as libX11 does.
///
/// Case: a script appends an alpha component, writing
/// `rgb:ff/ff/ff/ff`, or drops the blue one.
#[test]
fn an_rgb_string_without_exactly_three_components_is_refused() {
    for spec in [b"rgb:".as_slice(), b"rgb:ff/ff", b"rgb:ff/ff/ff/ff"] {
        assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
    }
}
```

## TC-10 — 成分が 0 桁または 5 桁なら拒否する

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec` に `b"rgb:/ff/ff"`、`b"rgb:ff//ff"`、`b"rgb:fffff/0/0"` |
| Expect | すべて `None` を返す **[C2]** ／ 定義されている桁数は 1〜4 だけ **[C3]** |

成分は `h | hh | hhh | hhhh` の 4 通りしかなく、0 桁と 5 桁はその外側の境界にあたる。空成分を
0 として扱う実装と、5 桁目以降を切り捨てて 4 桁として読む実装を捕まえる。先頭・中間の両方で
空成分を試すのは、区切り方によってどちらか一方しか落ちない実装があるため。

```rust
/// Asserts that an empty or five-digit `rgb:` component is refused.
///
/// Case: a script loses a component to an empty variable, or pads one
/// to five digits.
#[test]
fn an_empty_or_over_long_rgb_component_is_refused() {
    for spec in [b"rgb:/ff/ff".as_slice(), b"rgb:ff//ff", b"rgb:fffff/0/0"] {
        assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
    }
}
```

## TC-11 — 16 進でないバイトは拒否する

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec` に `b"rgb:fg/00/00"`、`b"rgb:+f/0/0"`、`b"#ggg"` |
| Expect | すべて `None` を返す **[C2]** |

成分は 16 進数字だけで構成される。`+f` を入れてあるのは、`from_str_radix` や `parse` に丸投げ
した実装が符号を受け付けてしまい、`rgb:+f/0/0` を色として通してしまうため。両形式で試すのは、
`rgb:` 側と `#` 側で桁の読み取りが別経路になりうるから。

```rust
/// Asserts that a byte other than a hex digit is refused in either
/// form, a sign included.
///
/// Case: a typo puts a `g` or a `+` into a theme colour.
#[test]
fn a_non_hex_digit_is_refused() {
    for spec in [b"rgb:fg/00/00".as_slice(), b"rgb:+f/0/0", b"#ggg"] {
        assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
    }
}
```

## TC-12 — `rgb:` 以外の color space は拒否する

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec` に `b"rgbi:1.0/0.0/0.0"`、`b"CIEXYZ:0.3227/0.28133/0.2493"`、`b""` |
| Expect | すべて `None` を返す **[C2]** |

数値指定は `<color_space_name>:<value>/.../<value>` の形を共有しており、`rgb:` はその 1 つでしか
ない。`rgbi:` は先頭 3 文字が同じなので、prefix を `starts_with(b"rgb")` で見る実装がここで落ちる。
空文字列は prefix が無い側の境界。

```rust
/// Asserts that the other Xlib colour spaces and an empty spec are
/// refused.
///
/// Case: a script uses Xlib's device-independent spellings such as
/// `rgbi:1.0/0.0/0.0`, or hands over an empty string.
#[test]
fn other_color_spaces_are_refused() {
    for spec in [
        b"rgbi:1.0/0.0/0.0".as_slice(),
        b"CIEXYZ:0.3227/0.28133/0.2493",
        b"",
    ] {
        assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
    }
}
```

## TC-A1 — 前後の空白は削らずに拒否する

| | |
| - | - |
| Setup | なし（純粋関数） |
| Act | `Rgb::from_color_spec` に `b" rgb:ff/ff/ff"`、`b"rgb:ff/ff/ff "`、`b" #fff"` |
| Expect | すべて `None` を返す **[W]** |

このケースだけはマニュアルに根拠が無く、doc comment が「前後の空白は削らない」と宣言している
ことだけを根拠にしている（だから `TC-A` 番号を振ってある）。先頭の空白は prefix 比較で、末尾の
空白は最後の成分の 16 進検査で落ちるので、経路が違う 2 つを両方確かめる。

```rust
/// Asserts that surrounding whitespace is refused rather than trimmed.
///
/// Case: a script builds the spec from a padded shell variable.
#[test]
fn surrounding_whitespace_is_refused_rather_than_trimmed() {
    for spec in [b" rgb:ff/ff/ff".as_slice(), b"rgb:ff/ff/ff ", b" #fff"] {
        assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
    }
}
```

---

## 付録

### 契約表

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C1 | Color Strings | unconditional | Color strings are case-insensitive. | xlib.pdf p.89, L5027-5029 |
| C2 | RGB Device String Specification | unconditional | `rgb:<red>/<green>/<blue>`, `<red>, <green>, <blue> := h \| hh \| hhh \| hhhh` | xlib.pdf p.89, L5075-5077 |
| C3 | RGB Device のスケール | bounded value | Note that h indicates the value scaled in 4 bits, hh the value scaled in 8 bits, hhh the value scaled in 12 bits, and hhhh the value scaled in 16 bits, respectively. | xlib.pdf p.90, L5086-5087 |
| C4 | RGB Device の幅混在 | unconditional | but mixed numbers of hexadecimal digit strings are also allowed | xlib.pdf p.90, L5089-5091 |
| C5 | 旧 `#` 形式 | bounded value | #RGB (4 bits each) / #RRGGBB (8 bits each) / #RRRGGGBBB (12 bits each) / #RRRRGGGGBBBB (16 bits each) | xlib.pdf p.90, L5100-5103 |
| C6 | 旧 `#` 形式の意味 | conditional | unlike the ``rgb:'' syntax, in which values are scaled … For example, the string ``#3a7'' is the same as ``#3000a0007000''. | xlib.pdf p.90, L5106-5108 |
| C7 | OSC 4 の spec パラメータ | unconditional | The spec can be a name or RGB specification as per XParseColor. | xterm-ctlseqs.pdf p.38, L2062-2063 |

kind-2 台帳:

```
N — crates/orzma_vt/src/device/color.rs:113, Rgb::from_color_spec
    "16-bit value is then narrowed to its high byte, the channel width"
    justifies: 各ケースの Expect が 8 ビット値を名指しすること

W — crates/orzma_vt/src/device/color.rs:120-121, Rgb::from_color_spec
    "Surrounding whitespace is" / "not trimmed."
    justifies: TC-A1

S — crates/orzma_vt/src/device/color.rs:119-120, Rgb::from_color_spec
    "an `rgb:` string with anything after its third component is" / "refused, where libX11 ignores the tail."
    justifies: TC-09 の 4 成分を拒否する行
```

C6 の quote が 2 文に分かれているのは、間の "most significant bits" を含む文が `pdftotext` の
出力で ﬁ / ﬀ の合字になり、検証スクリプトの語分割と噛み合わないため。合字を含まない範囲で
同じ内容（スケールしない・`#3a7` の例）を引いている。

### 探索した語

| 語 | Level | 結果 |
| - | - | - |
| `Xlib color string` / `Color Strings` | 1 | xlib.pdf L5026 に節そのもの。C1 に到達 |
| `RGB Device` | 1 | xlib.pdf L5070 の節見出し。C2・C3・C4 に到達 |
| `rgb:` / `rgb:<red>` | 1, 4 | xlib.pdf L5071-5077。C2 に到達 |
| `#RGB` / `#RRRRGGGGBBBB` | 1 | xlib.pdf L5100-5103。C5 に到達 |
| `most significant bits` / `3a7` | 1 | xlib.pdf L5105-5108。C6 に到達（合字のため引用範囲を調整） |
| `scaled in` | 1 | xlib.pdf L5086-5087。C3 に到達 |
| `case-insensitive` | 1 | xlib.pdf L5029。C1 に到達 |
| `XParseColor` | 1, 4 | xterm-ctlseqs.pdf L2062-2063、L2106。C7 に到達 |
| `color_space_name` | 4 | xlib.pdf L5057。数値指定の共通形として TC-12 の背景 |
| `RGB Intensity` / `rgbi:` | 4 | xlib.pdf L5110-5115。別の color space であることを確認（TC-12 の対象） |
| `CIEXYZ` / `CIELuv` | 4 | xlib.pdf L5062-5065 の例。同上 |
| `libX11` | 1 | doc comment 由来の語。マニュアル本文には節名として現れず、振る舞いの記述には到達しなかった |
| `sRGB` / `24-bit` | 2 | `Rgb` の doc（"A 24-bit sRGB color."）由来。どのマニュアルでも振る舞いの記述に到達しなかった |
| `palette` / `SGR` | 3 | ファイルの `//!` 由来。色文字列の文法には到達しなかった（OSC 4 側の話であり C7 で足りる） |

ECMA-48・vt220・vt510 は色文字列の文法を扱っておらず、`rgb:` でも `XParseColor` でも振る舞いの
記述に到達しなかった。1e の順位はこの 3 冊のあいだのもので、今回の根拠はその外側にある
xlib.pdf と xterm-ctlseqs.pdf に閉じている。

### 仕様にないもの

- **8 ビットへ狭める規則**: どのマニュアルも色を 16 ビットで定義しており、8 ビットへ丸める操作は
  規定していない。タグ **N**（リポジトリの doc comment）が唯一の根拠で、ほぼ全ケースの Expect に
  併記してある。仕様から導けるのは 16 ビットの中間値までで、そこから先は orzma の決定である。
- **前後の空白の扱い**: マニュアルに記述が無い（TC-A1）。
- **3 成分より後ろの文字**: libX11 の実装は読み捨てるが、マニュアルの文法は 3 成分ちょうどを
  定めている。orzma は拒否する側を選んでおり、TC-09 は C2 の文法に立ち、その差分をタグ **S** が
  記録している。

### 仕様の矛盾

1 件あり、ケースは出していない。

- **C7（検証済み、xterm-ctlseqs.pdf p.38 L2062-2063）**: "The spec can be a name or RGB
  specification as per XParseColor."
- **リポジトリの doc comment（crates/orzma_vt/src/device/color.rs:121-123）**: "Color names and the
  device-independent color spaces (`rgbi:`, `CIEXYZ:`, and the rest) are refused, because this
  terminal ships no color name database."

マニュアルは色名を spec として認め、doc comment は拒否すると宣言している。リポジトリの記述が
検証済みのマニュアルの文を覆すことはできないので、「色名を拒否する」ケースはこのリストに
入っていない。色名を実装しない判断そのものは設計（spec の決定 4）で下されており、仕様からの
意図的な逸脱として扱うのが筋である。なお `rgbi:` と `CIEXYZ:` の拒否は色名とは別で、C2 の
prefix が `rgb:` であることから導けるため TC-12 として残してある。

### API 改定

無し。`from_color_spec(spec: &[u8]) -> Option<Self>` は上の 13 ケースすべてを表現できる。

### 出典なしの改善提案

無し。

### docs/todo との衝突

無し。`docs/todo/` を `from_color_spec` / `color_spec` / `XParseColor` / `rgbi` / `Rgb::` で検索し、
一致する記述は見つからなかった。
