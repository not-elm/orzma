# Test cases: KeyText::encode

状態: 承認済み

対象は `KeyText::encode`（`crates/orzma_tty/src/input/keyboard/text.rs:31`、`pub fn encode(&self, mods: &TerminalModifiers) -> Vec<u8>`）。参照した仕様は `vt220.pdf`（3.2.1 の Ctrl の説明、表 3-5）と `xterm-ctlseqs.pdf`（Alt and Meta Keys、DECSET 1036・1039、modifyOtherKeys）。`vt510.pdf` には Ctrl で制御文字を作る表が見つからなかった。citation は 18/18 件（説明書 13 件、リポジトリの doc コメント 5 件）を検証した。ケースは 8 件で、自明が 1 件、要確認が 7 件。API 改定の提案は 1 組（P ブロッカー 3 件）。

作者は 2026-10-22 にこの会話で、`Ctrl+/` と `Ctrl+?` は VT220 の表 3-5（`/` は FS、`?` は US）ではなく、要件 `docs/requirements/2026-10-21-xterm-key-encoding.md` の R7 どおり US と DEL にすると決めた。表 3-5 にない `@`・`\`・`^`・`_` などは、メソッドの doc コメントを出典（kind 2）にしてよい。

この一覧は仕様から導いて自分で検証したもので、作者が 2026-10-22 に読み、会話で承認した。

ケースは High → Medium の順に並べている。High だけ読めば、仕様がはっきり述べている振る舞いはすべて含まれる。

## テストケース一覧

| # | 名前 | Source | 優先度 | 確認 |
| - | - | - | - | - |
| TC-01 | `ctrl_with_a_letter_sends_its_c0_byte_in_either_case` | T2, T4, KL | High | 要確認 |
| TC-02 | `ctrl_with_a_vt220_control_key_sends_its_control_character` | T3 | High | 自明 |
| TC-03 | `alt_or_meta_without_ctrl_prefixes_an_ordinary_character_with_esc` | A2, A3 | Medium | 要確認 |
| TC-A1 | `ctrl_with_slash_sends_us_and_ctrl_with_question_mark_sends_del` | KS | Medium | 要確認 |
| TC-A2 | `ctrl_with_an_ascii_control_symbol_sends_its_control_character` | KS | Medium | 要確認 |
| TC-A3 | `ctrl_with_alt_or_meta_prefixes_the_control_character_with_esc` | KE | Medium | 要確認 |
| TC-A4 | `ctrl_with_text_that_has_no_control_character_sends_the_text` | KC, KO | Medium | 要確認 |
| TC-A5 | `non_ascii_text_is_sent_as_utf8` | KO | Medium | 要確認 |

Source タグ — **T2**: vt220.pdf p.75 表 3-5（Ctrl と英字で SOH〜SUB）／**T3**: p.75 表 3-5（Ctrl と 2・Space・3・`[`・4・5・`]`・6・`~`・7・8 で NUL〜DEL）／**T4**: p.75 表 3-5 の注（Ctrl-S と Ctrl-Q は XOFF のサポートが無効のときだけ）／**A2**・**A3**: xterm-ctlseqs.pdf p.44（Alt and Meta Keys の表）／**KC**・**KL**・**KS**・**KE**・**KO**: `KeyText::encode` の doc コメント（下の台帳）。全文は付録の契約表にある。

TC-01〜TC-03 は説明書がそのまま述べているケース。`TC-A` の 5 件は、`docs/references/` のどの説明書も述べていない振る舞いを守るケースで、doc コメントだけを出典にしている。その内訳は 3 つある。

- TC-A1：VT220 の表と食い違う。作者が要件どおりにすると決めた。
- TC-A2：表にない記号で、ASCII の慣例から来ている。
- TC-A3・TC-A4・TC-A5：Ctrl と Alt の組み合わせと、制御文字にならない文字の扱い。

TC-02 は、表 3-5 の 1 つの契約エントリ（T3）だけから来る unconditional のケースなので自明にした。Rust では制御文字を 16 進のバイト値で書いている（表 3-5 の `0/00` などの列・行の表記を 16 進に直したもの）。

テストコードは `crates/orzma_tty/src/input/keyboard/text.rs` の `#[cfg(test)] mod tests` に追加する。このモジュールには `KeyText::new` のテストが 2 件あるが、ヘルパはないので、`text`・`held`・`ctrl` などのヘルパを新しく置く。`keyboard.rs` の `mod tests` にある `ctrl_letter_collapses_to_c0_byte` などが試しているのは `encode_key` で、`KeyText` そのものではないので、そちらには置かない。今の木にあるシグネチャに対して書いているのでコンパイルは通るはずだが、このスキルはビルドしていない。本体が `todo!()` なので、実装するまではテストは panic して失敗する。

### kind 2 の台帳

```
KC — crates/orzma_tty/src/input/keyboard/text.rs:24-25, KeyText::encode
     "With Ctrl held, a single character that has a control character sends that byte instead."
     justifies: 2 文字以上のテキストは Ctrl でも制御文字にしないこと
KL — crates/orzma_tty/src/input/keyboard/text.rs:25-26, KeyText::encode
     "A letter in either case sends its C0 byte, so `a` and `A` both send SOH."
     justifies: 小文字と大文字が同じ C0 バイトを送ること
KS — crates/orzma_tty/src/input/keyboard/text.rs:26-28, KeyText::encode
     "`Space`, `@`, and `2` send NUL; `[` and `3` send ESC; `\` and `4` send FS; `]` and `5` send GS; `^`, `~`, and `6` send RS; `_`, `/`, and `7` send US; and `?` and `8` send DEL."
     justifies: 表 3-5 にない `@`・`\`・`^`・`_` の制御文字と、表と違う `/`（US）・`?`（DEL）
KE — crates/orzma_tty/src/input/keyboard/text.rs:28-29, KeyText::encode
     "With Alt or Meta also held, `ESC` comes before that byte."
     justifies: Ctrl と Alt（または Meta）を一緒に押すと、制御文字の前に ESC が付くこと
KO — crates/orzma_tty/src/input/keyboard/text.rs:29-30, KeyText::encode
     "Any other text is sent as UTF-8, after `ESC` when Alt or Meta is held."
     justifies: 制御文字にならないテキストは UTF-8 のまま送り、Alt か Meta があれば ESC を前に付けること
```

## TC-01 — Ctrl と英字は、大文字でも小文字でもその英字の C0 バイトを送る

| | |
| - | - |
| Setup | `TerminalModifiers { ctrl: true, .. }` |
| Act | `a`〜`z` と `A`〜`Z` のそれぞれの `KeyText` を `encode` する |
| Expect | `A`〜`Z` は SOH（0x01）〜SUB（0x1A）**[T2]** ／ 小文字は同じ英字の大文字と同じバイト **[KL]** ／ `S` と `Q` も DC3（0x13）と DC1（0x11）を送る **[T2][T4]** |

表 3-5（T2）は「CTRL と一緒に押すキー」として英字のキーを挙げていて、Shift を押しているかどうかは区別していない。それを、論理キーが小文字で届くか大文字で届くかにかかわらず同じバイトを送る、という形にしたのは doc コメント（KL）。T4 は、Ctrl-S と Ctrl-Q が制御文字を送るのは XOFF のサポートが無効のときだけだと述べている。orzma は XOFF を自分では扱わずに PTY に渡すので、無効の側として書いた（有効の側は付録「API 改定」の B3）。26 文字すべてを回すのは、`c - 'a' + 1` のような計算で作る実装の境目（`z`）を落とさないため。

```rust
/// Asserts that Ctrl with a letter of either case sends that letter's C0
/// byte, from SOH for `a` to SUB for `z`.
///
/// Case: the user presses Ctrl-C to interrupt a command, or Ctrl-Shift-A in
/// a program that receives the letter in uppercase.
#[test]
fn ctrl_with_a_letter_sends_its_c0_byte_in_either_case() {
    for (byte, lower) in (1u8..).zip('a'..='z') {
        let upper = lower.to_ascii_uppercase();
        assert_eq!(text(&lower.to_string()).encode(&ctrl()), [byte], "{lower}");
        assert_eq!(text(&upper.to_string()).encode(&ctrl()), [byte], "{upper}");
    }
}
```

## TC-02 — Ctrl と、表 3-5 にある数字・記号のキーは、表の制御文字を送る

| | |
| - | - |
| Setup | `TerminalModifiers { ctrl: true, .. }` |
| Act | `Space`・`2`・`3`・`[`・`4`・`5`・`]`・`6`・`~`・`7`・`8` のそれぞれの `KeyText` を `encode` する |
| Expect | `Space` と `2` は NUL（0x00）、`3` と `[` は ESC（0x1B）、`4` は FS（0x1C）、`5` と `]` は GS（0x1D）、`6` と `~` は RS（0x1E）、`7` は US（0x1F）、`8` は DEL（0x7F）**[T3]** |

表 3-5 の英字以外の行（T3）のうち、要件と食い違わないものをそのまま並べた。食い違う `/` と `?` は TC-A1 に分けた。数字の 2〜8 は R8 の要件とも一致する。防ぎたいのは、英字だけを扱う今の実装と、`c & 0x1F` のような計算を数字にも当てて `2` で 0x12 を出す実装。

```rust
/// Asserts that Ctrl with `Space`, `2` to `8`, `[`, `]`, or `~` sends the
/// control character the VT220 assigns to that key.
///
/// Case: the user presses Ctrl-Space to set the mark in Emacs, Ctrl-[ to
/// leave insert mode in vim, or Ctrl-6 to switch to the alternate file.
#[test]
fn ctrl_with_a_vt220_control_key_sends_its_control_character() {
    let cases: [(&str, u8); 11] = [
        (" ", 0x00),
        ("2", 0x00),
        ("3", 0x1b),
        ("[", 0x1b),
        ("4", 0x1c),
        ("5", 0x1d),
        ("]", 0x1d),
        ("6", 0x1e),
        ("~", 0x1e),
        ("7", 0x1f),
        ("8", 0x7f),
    ];
    for (key, byte) in cases {
        assert_eq!(text(key).encode(&ctrl()), [byte], "{key:?}");
    }
}
```

## TC-03 — Ctrl なしの Alt か Meta は、ふつうの文字の前に ESC を付ける

| | |
| - | - |
| Setup | 修飾キーなし、Meta だけ、Alt だけ、Alt と Meta の 4 通りの `TerminalModifiers` |
| Act | `KeyText` の `x` をそれぞれで `encode` する |
| Expect | 修飾キーなしは `x` ／ Meta は `ESC x` ／ Alt は `ESC x` ／ Alt と Meta は `ESC x` **[A2][A3]** |

xterm の表（A3）は altSendsEscape と metaSendsEscape がどちらも ON のときの結果で、orzma の今の振る舞いはこの行にあたる。表は 4 通りの resource の組み合わせごとに結果を出しているので（mode-dependent）、優先度を Medium にした。両方とも off の側（8 ビット目を立てる）は書けない（付録「API 改定」の B1）。防ぎたいのは、Meta を Alt と違うものとして扱って ESC を付けない実装。

```rust
/// Asserts that Alt or Meta held without Ctrl sends `ESC` before an
/// ordinary character, and that the character alone is sent unchanged.
///
/// Case: the user presses Alt-b to move back one word in a shell, or types
/// a plain letter with no modifier held.
#[test]
fn alt_or_meta_without_ctrl_prefixes_an_ordinary_character_with_esc() {
    assert_eq!(text("x").encode(&TerminalModifiers::default()), b"x");
    assert_eq!(text("x").encode(&meta()), b"\x1bx");
    assert_eq!(text("x").encode(&alt()), b"\x1bx");
    assert_eq!(text("x").encode(&alt_meta()), b"\x1bx");
}
```

## TC-A1 — Ctrl と `/` は US を、Ctrl と `?` は DEL を送る

| | |
| - | - |
| Setup | `TerminalModifiers { ctrl: true, .. }` |
| Act | `/` と `?` の `KeyText` を `encode` する |
| Expect | `/` は US（0x1F）、`?` は DEL（0x7F）**[KS]** |

表 3-5（T3）は `/` に FS を、`?` に US を割り当てていて、doc コメント（KS）と食い違う。説明書と食い違う kind 2 からはふつうケースを出さないが、作者が要件の R7 どおりにすると決めたので、doc コメントだけを出典にした `TC-A` として出した（付録「仕様の矛盾」）。防ぎたいのは、表 3-5 をそのまま写して `/` で FS を出す実装。

```rust
/// Asserts that Ctrl with `/` sends US and Ctrl with `?` sends DEL, rather
/// than the FS and US the VT220 table assigns to those keys.
///
/// Case: the user presses Ctrl-/ to undo in Emacs, or Ctrl-? in a program
/// that reads DEL as Backspace.
#[test]
fn ctrl_with_slash_sends_us_and_ctrl_with_question_mark_sends_del() {
    assert_eq!(text("/").encode(&ctrl()), [0x1f]);
    assert_eq!(text("?").encode(&ctrl()), [0x7f]);
}
```

## TC-A2 — Ctrl と `@`・`\`・`^`・`_` は、NUL・FS・RS・US を送る

| | |
| - | - |
| Setup | `TerminalModifiers { ctrl: true, .. }` |
| Act | `@`・`\`・`^`・`_` の `KeyText` を `encode` する |
| Expect | `@` は NUL（0x00）、`\` は FS（0x1C）、`^` は RS（0x1E）、`_` は US（0x1F）**[KS]** |

表 3-5 にはこの 4 つの記号がなく、`docs/references/` のほかの説明書にも、Ctrl とこれらの記号の組み合わせを述べた記述はなかった。ASCII で下位 5 ビットだけを残すという慣例から来る割り当てで、doc コメント（KS）だけを出典にした `TC-A`。防ぎたいのは、表 3-5 にある記号だけを扱って、これらを記号のまま送る実装。

```rust
/// Asserts that Ctrl with `@`, `\`, `^`, or `_` sends NUL, FS, RS, or US.
///
/// Case: the user presses Ctrl-\ to send SIGQUIT, Ctrl-_ to undo in
/// readline, or Ctrl-^ to switch to the alternate file in vim.
#[test]
fn ctrl_with_an_ascii_control_symbol_sends_its_control_character() {
    let cases: [(&str, u8); 4] = [("@", 0x00), ("\\", 0x1c), ("^", 0x1e), ("_", 0x1f)];
    for (key, byte) in cases {
        assert_eq!(text(key).encode(&ctrl()), [byte], "{key:?}");
    }
}
```

## TC-A3 — Ctrl と Alt（または Meta）は、Ctrl だけのときの制御文字の前に ESC を付ける

| | |
| - | - |
| Setup | Ctrl と Alt の `TerminalModifiers`、Ctrl と Meta の `TerminalModifiers` |
| Act | Ctrl と Alt で `f`・`F`・`Space`・`[`・`?`・`8` を、Ctrl と Meta で `a` を `encode` する |
| Expect | `f` と `F` は `ESC` ACK（`ESC 0x06`）、`Space` は `ESC 0x00`、`[` は `ESC ESC`、`?` と `8` は `ESC 0x7F` ／ Ctrl と Meta の `a` は `ESC 0x01` **[KE]** |

xterm の Alt and Meta Keys の表（A3）は、ふつうの文字「x」に Alt か Meta を足したときの結果だけを述べていて、Ctrl と一緒のときは述べていない。doc コメント（KE）だけを出典にした `TC-A`。英字（`f`、`F`、`a`）、表 3-5 のキー（`Space`、`[`、`8`）、doc コメントで決めたキー（`?`）を 1 つずつ入れて、どの経路の制御文字にも ESC が付くことを確かめる。防ぎたいのは、Ctrl の制御文字を返したところで処理を終え、ESC を付けない今の実装。

```rust
/// Asserts that Ctrl with Alt or Meta sends `ESC` before the control
/// character Ctrl alone sends.
///
/// Case: the user presses Ctrl-Alt-f to move forward over an expression in
/// Emacs, or Ctrl-Alt-h to delete the previous word in readline.
#[test]
fn ctrl_with_alt_or_meta_prefixes_the_control_character_with_esc() {
    let cases: [(&str, &[u8]); 6] = [
        ("f", b"\x1b\x06"),
        ("F", b"\x1b\x06"),
        (" ", b"\x1b\x00"),
        ("[", b"\x1b\x1b"),
        ("?", b"\x1b\x7f"),
        ("8", b"\x1b\x7f"),
    ];
    for (key, expected) in cases {
        assert_eq!(text(key).encode(&ctrl_alt()), expected, "{key:?}");
    }
    assert_eq!(text("a").encode(&ctrl_meta()), b"\x1b\x01");
}
```

## TC-A4 — 制御文字にならないテキストは、Ctrl を押していてもそのまま送る

| | |
| - | - |
| Setup | `TerminalModifiers { ctrl: true, .. }`、Ctrl と Alt の `TerminalModifiers` |
| Act | Ctrl で `1`・`9`・`0`・`-`・`あ`・`ab` を、Ctrl と Alt で `1` を `encode` する |
| Expect | Ctrl だけのときは、どれもテキストの UTF-8 のまま **[KC][KO]** ／ Ctrl と Alt の `1` は `ESC 1` **[KO]** |

R8 は `2`〜`8` だけを対象にしていて、`1`・`9`・`0` は今までどおりにする。`-` も R7 の対象外。`ab` は 2 文字なので、先頭が英字でも制御文字にしない（KC）。防ぎたいのは、`c & 0x1F` のような計算を数字や記号すべてに当てる実装（`1` で 0x11 を出す）と、テキストの先頭の 1 文字だけを見て制御文字にする実装。

```rust
/// Asserts that Ctrl with text that has no control character sends the
/// text unchanged, after `ESC` when Alt is also held.
///
/// Case: the user presses Ctrl-1, Ctrl-9, or Ctrl-- in a shell, keys that
/// have no control character of their own.
#[test]
fn ctrl_with_text_that_has_no_control_character_sends_the_text() {
    for key in ["1", "9", "0", "-", "あ", "ab"] {
        assert_eq!(text(key).encode(&ctrl()), key.as_bytes(), "{key:?}");
    }
    assert_eq!(text("1").encode(&ctrl_alt()), b"\x1b1");
}
```

## TC-A5 — ASCII でないテキストは UTF-8 で送る

| | |
| - | - |
| Setup | 修飾キーなし、Alt だけの `TerminalModifiers` |
| Act | 修飾キーなしで `あ` と `e` + 結合用アクセント（U+0301）を、Alt で `あ` を `encode` する |
| Expect | 修飾キーなしは、どちらもテキストの UTF-8 のまま ／ Alt の `あ` は `ESC` のあとに `あ` の UTF-8 **[KO]** |

xterm の表（A3）が述べているのは ASCII の「x」までで、ASCII でない文字をどう送るかは `docs/references/` の説明書に書かれていなかった。doc コメント（KO）だけを出典にした `TC-A`。結合文字の列を入れるのは、`KeyText` が 1 つの論理キーで複数のコードポイントを運ぶことがあるため。防ぎたいのは、最初の `char` だけを送る実装と、Alt のときに UTF-8 の各バイトに ESC を付ける実装。

```rust
/// Asserts that non-ASCII text is sent as UTF-8, after `ESC` when Alt is
/// held.
///
/// Case: the user types `あ` or a composed `é` through an input method, or
/// presses Alt with a key that types a non-ASCII letter.
#[test]
fn non_ascii_text_is_sent_as_utf8() {
    for key in ["あ", "e\u{301}"] {
        assert_eq!(
            text(key).encode(&TerminalModifiers::default()),
            key.as_bytes(),
            "{key:?}"
        );
    }
    let mut expected = vec![0x1b];
    expected.extend_from_slice("あ".as_bytes());
    assert_eq!(text("あ").encode(&alt()), expected);
}
```

テスト用のヘルパ（`mod tests` の中に置く）:

```rust
fn text(s: &str) -> KeyText {
    KeyText::new(s).expect("test text must be non-empty")
}

fn held(ctrl: bool, alt: bool, meta: bool) -> TerminalModifiers {
    TerminalModifiers {
        ctrl,
        shift: false,
        alt,
        meta,
    }
}

fn ctrl() -> TerminalModifiers {
    held(true, false, false)
}

fn alt() -> TerminalModifiers {
    held(false, true, false)
}

fn meta() -> TerminalModifiers {
    held(false, false, true)
}

fn alt_meta() -> TerminalModifiers {
    held(false, true, true)
}

fn ctrl_alt() -> TerminalModifiers {
    held(true, true, false)
}

fn ctrl_meta() -> TerminalModifiers {
    held(true, false, true)
}
```

## 付録

### 契約表

`pdftotext -layout`（Git for Windows に入っている xpdf の 4.00）で抽出したテキストの行番号。ページは PDF のページ番号を計算したもの。表の行が 10 行を超えるエントリは、`verify` の幅の上限に合わせて範囲を分け、すべてを検証した。

| ID | Governs | Shape | Statement | Citation |
| --- | --- | --- | --- | --- |
| T0 | 表 3-5 Keys Used to Generate 7-Bit Control Characters | unconditional | 表の見出しと列見出し "Control Character Mnemonic / Code / Key Pressed with CTRL (All Modes) / Dedicated Function Key" | vt220.pdf p.75, L4757-4761 |
| T2 | 表 3-5 | unconditional | SOH 0/01 A, STX 0/02 B, ETX 0/03 C, EOT 0/04 D, ENQ 0/05 E ／ ACK F, BEL G, BS H, HT I, LF J ／ VT K, FF L, CR M, SO N, SI O ／ DLE P, DC1 Q, DC2 R, DC3 S, DC4 T ／ NAK U, SYN V, ETB W, CAN X, EM Y ／ SUB 1/10 Z | vt220.pdf p.75, L4765-4773 ／ L4775-4783 ／ L4785-4793 ／ L4795-4803 ／ L4805-4813 ／ L4815 |
| T3 | 表 3-5 | unconditional | NUL 0/00 2, space ／ ESC 1/11 3,[ ; FS 1/12 4,/ ; GS 1/13 5,] ; RS 1/14 6,~ ; US 1/15 7,? ／ DEL 7/15 8 | vt220.pdf p.75, L4763 ／ L4817-4825 ／ L4827 |
| T4 | 表 3-5 の注 | conditional | "** These keystrokes are enabled only if XOFF support is disabled. If XOFF support is enabled, then CTRL-S is a "hold screen" local function and CTRL-Q is an "unhold screen" local function." | vt220.pdf p.75, L4832-4833 |
| V22 | 3.2.1 Main Keypad（Ctrl） | unconditional | "The Ctrl key alone does not send a code. It is always used in combination with another key to send a control code." | vt220.pdf p.18, L762-763 |
| A1 | Alt and Meta Keys | mode-dependent | "The metaSendsEscape resource (and altSendsEscape if altIsNotMeta is set) can be used to control the way the Meta modifier applies to ordinary keys unless the modifyOtherKeys resource is set: o prefix a key with the ESC character. o shift the key from codes 0-127 to 128-255 by adding 128." | xterm-ctlseqs.pdf p.43, L2489-2495 |
| A2 | Alt and Meta Keys | mode-dependent | "The table shows the result for a given character "x" with modifiers according to the default translations with the resources set on or off. This assumes altIsNotMeta is set: key altSendsEscape metaSendsEscape result" | xterm-ctlseqs.pdf p.44, L2496-2500 |
| A3 | Alt and Meta Keys | mode-dependent | x ON ON x, Meta-x ON ON ESC x, Alt-x ON ON ESC x, Alt+Meta-x ON ON ESC x | xterm-ctlseqs.pdf p.44, L2528-2534 |
| A4 | Alt and Meta Keys | mode-dependent | x off off x, Meta-x off off shift, Alt-x off off shift, Alt+Meta-x off off shift（B1 の提案でだけ使う） | xterm-ctlseqs.pdf p.44, L2504-2510 |
| M3 | DECSET 1036 | mode-dependent | "Ps = 1 0 3 6 Send ESC when Meta modifies a key, xterm. This enables the metaSendsEscape resource."（B1 の提案でだけ使う） | xterm-ctlseqs.pdf p.20, L1113-1114 |
| M4 | DECSET 1039 | mode-dependent | "Ps = 1 0 3 9 Send ESC when Alt modifies a key, xterm. This enables the altSendsEscape resource, xterm."（B1 の提案でだけ使う） | xterm-ctlseqs.pdf p.20, L1117-1118 |
| MO | modifyOtherKeys | mode-dependent | "When modifyOtherKeys is set, ordinary keys may be sent as escape sequences:"（B2 の提案でだけ使う） | xterm-ctlseqs.pdf p.44, L2538-2539 |
| MO2 | modifyOtherKeys | mode-dependent | "When modifyOtherKeys is set to 1, the usual shift- and control- modifiers work as expected, but other modifiers (such as alt- and meta-modifiers) cause ordinary keys to be encoded as if they were function-keys. For example, alt-Tab sends CSI 2 7 ; 3 ; 9 ~ (the second parameter is "3" for alt, and the third parameter is the ASCII value of tab, "9")."（B2 の提案でだけ使う） | xterm-ctlseqs.pdf p.44, L2541-2546 |
| KC・KL・KS・KE・KO | `KeyText::encode` の doc コメント | — | kind 2。上の台帳を参照 | text.rs:24-30 |

citations verified: 18/18（説明書 13 件は範囲 20 個すべて `verify`、doc コメント 5 件は `grep -F`）。T0 は最初、見出しの語を列の意味の順に並べた引用で記録して、`verify` が棄却した（REJECTED、14/24 matched）。`-layout` の抽出では 2 段の列見出しが行ごとに交互に並ぶので、抽出したテキストの実際の並び（"Control Code Key Pressed with Dedicated Function Key Character CTRL (All Modes) Mnemonic"）で記録し直し、再検証を通した。契約表の Statement には、意味の順に組み直したものを書いている。T3 の FS の行の `/` は、`pdftotext -raw` で同じページを抽出し直しても `/` だった。

手順の中での順位づけ:

- **Ctrl で作る制御文字**：VT510 で `Ctrl key`・`CTRL key`・`Ctrl/`・`Ctrl-`・`control character` を検索したが、Ctrl と文字キーで作る制御文字の表はなかった（見つかったのは Ctrl/Alt/F1 などのローカル機能と、DECKPM の説明）。そのため、順位が次の VT220 の表 3-5 を採った。ECMA-48 はキーボードからの入力を扱わない。
- **`/` と `?`**：表 3-5（T3）と doc コメント（KS）が食い違う。作者の決定で doc コメント（要件 R7）を採った（「仕様の矛盾」）。

### 探索した語

| 語 | レベル | 結果 |
| --- | --- | --- |
| Ctrl / control character / C0 | 1（メソッドの doc） | vt220 の 3.2.1（V22）と表 3-5（T0〜T4） |
| NUL / ESC / FS / GS / RS / US / DEL / SOH | 1 | vt220 表 3-5（T2、T3） |
| Alt / Meta / ESC | 1 | xterm-ctlseqs p.43-44 の Alt and Meta Keys（A1〜A4）、DECSET 1036・1039（M3、M4） |
| UTF-8 | 1 | キーボードからの入力について述べた記述はなかった |
| typed text / KeyText | 2・3（struct の doc と `//!`） | 新しい語はなかった |
| modifyOtherKeys | A1 の条件の中で出た語 | xterm-ctlseqs p.44（MO、MO2） |
| XOFF | T4 の注で出た語 | vt220 表 3-5 の注（T4） |
| Ctrl key / CTRL key / Ctrl/ / Ctrl- | VT510 が黙っているかの確認 | vt510 L1174・L1765（Ctrl/Alt/F1 などのキーボード言語の切り替え）、L7931（DECKPM）、L12713-12715・L13269-13291・L13308-13417（ローカル機能と SCO コンソールの表）。Ctrl と文字キーで作る制御文字の表はなかった |
| keyboard（ECMA-48） | ECMA-48 の確認 | L1595「This Standard does not deal with the input component.」の周辺だけで、キーボードの割り当てはない |

level 1 の語で振る舞いの記述に届いたので、level 4（型名 `TerminalModifiers` の展開）には降りていない。

定義でない行として棄却したもの（確認できたもの）:

- vt220：L4829-4830（F11〜F13 が制御文字を送るのは VT100・VT52 モードだけという注で、`KeyText` が受け持つキーではない）
- vt510：上の「探索した語」の行（Ctrl を使うローカル機能と、別のモードの表）
- xterm-ctlseqs：L45、L972、L1483（目次と相互参照）

### 仕様にないもの

- Ctrl と `@`・`\`・`^`・`_` の組み合わせ（TC-A2）。どの説明書にもなく、doc コメント（KS）だけを出典にした。
- Ctrl と Alt（または Meta）を一緒に押したときの結果（TC-A3）。xterm の表はふつうの文字「x」までしか述べていない。
- 制御文字にならないテキストを Ctrl で押したときと、ASCII でないテキストの扱い（TC-A4、TC-A5）。

### 仕様の矛盾

**Ctrl と `/`・`?`**（作者が 2026-10-22 に要件どおりにすると決めた）

| 出典 | `Ctrl+/` | `Ctrl+?` | 引用 |
| --- | --- | --- | --- |
| vt220 表 3-5 | FS（0x1C） | US（0x1F） | T3 |
| `KeyText::encode` の doc コメント（要件 R7） | US（0x1F） | DEL（0x7F） | KS |

説明書と食い違う kind 2 からはふつうケースを出さないが、作者は要件の R7 で doc コメントの側を選び、2026-10-22 にこの会話で、その前提でケースを出すよう求めた。TC-A1 はこの決定のもとで出したもので、表 3-5 の側（FS と US）のケースは出していない。要件の「検討した代替案」は、`Ctrl+?` を xterm 本体と同じく `?` のまま送る案を退けたことを記録しているが、表 3-5 との違いは記録していない。

### API 改定

下の 3 件は 1 組の変更として扱う。どれも P（パラメータがない）のブロッカー。今のシグネチャのままにすると、A1・A4 の「resource が off の側」、MO・MO2 の「modifyOtherKeys を設定した側」、T4 の「XOFF のサポートが有効の側」からはケースが出ない。

**B1 — altSendsEscape・metaSendsEscape が off の状態を作れない（A1、A4、M3、M4）**

- 書けない行：Setup「altSendsEscape と metaSendsEscape が off」。orzma_vt と orzma_tty には、DECSET 1036・1039 にあたるものが見当たらなかった。
- 最小の変更：`pub fn encode(&self, mods: &TerminalModifiers, alt_sends_escape: bool, meta_sends_escape: bool) -> Vec<u8>`。`VtModes` にモード 1036 と 1039 を加え、`encode_key` から渡す経路が要る。
- 影響するケース：
  - 今のシグネチャの TC-03：書ける（両方 ON の側）。
  - 提案のもとでの TC-03：Setup に両方 `true` を加えるだけで、Expect は同じ。
  - 提案のもとでの新しいケース（番号なし）：Setup 両方 `false` ／ Act Alt で `x` を `encode` ／ Expect `x` に 128 を足したコード **[A1][A4]**。UTF-8 の端末でこれをどう送るかは説明書に書かれていない。

**B2 — modifyOtherKeys を設定した状態を作れない（A1、MO、MO2）**

- 書けない行：Setup「modifyOtherKeys が 1 に設定されている」。
- 最小の変更：`pub fn encode(&self, mods: &TerminalModifiers, modify_other_keys: u8) -> Vec<u8>`。`CSI > 4 ; Pv m`（XTMODKEYS）で設定される値を VT の状態に持ち、`encode_key` から渡す経路が要る。
- 影響するケース：
  - 今のシグネチャ：設定した側は書けない。TC-03 は設定しない側として成り立つ。
  - 提案のもとでの新しいケース（番号なし）：Setup `modify_other_keys = 1` ／ Act Alt で `a` を `encode` ／ Expect `CSI 2 7 ; 3 ; 9 7 ~` **[MO2]**。MO2 の例は Tab なので、英字への当てはめは類推になる。
- 補足：要件ドキュメントは、修飾キー付きのキーを区別して送ることを kitty keyboard protocol の Issue で扱うとしている。

**B3 — XOFF のサポートが有効の状態を作れない（T4）**

- 書けない行：Setup「XOFF のサポートが有効」。Expect「Ctrl-S と Ctrl-Q は何も送らない」。
- 最小の変更：`pub fn encode(&self, mods: &TerminalModifiers, xoff_support: bool) -> Vec<u8>`。「何も送らない」は空の `Vec` で表せる。
- 影響するケース：
  - 今のシグネチャの TC-01：書ける（無効の側）。
  - 提案のもとでの新しいケース（番号なし）：Setup `xoff_support = true` ／ Act Ctrl で `s` と `q` を `encode` ／ Expect どちらも空の列 **[T4]**。

### 出典なしの改善提案

- `keyboard.rs` の `mod tests` にある次のテストは、`encode_key` を通して今回のケースと同じ約束を確かめている。どちらかに寄せると、同じ約束を 2 か所で守らずに済む。
  - `ascii_text_is_passthrough`・`multibyte_text_is_utf8_passthrough`（TC-03、TC-A5）
  - `ctrl_letter_collapses_to_c0_byte`・`ctrl_uppercase_letter_collapses_to_same_byte`（TC-01）
  - `ctrl_digit_is_text_passthrough`（TC-A4）
  - `alt_letter_is_esc_prefixed`・`meta_letter_is_esc_prefixed`（TC-03）
- 同じ `mod tests` の `ctrl_space_is_unmapped_in_mvp`（Ctrl+Space を空白のまま送る）と `ctrl_takes_priority_over_alt_for_letters`（Ctrl+Alt+a を SOH だけにする）は、TC-02 と TC-A3 と食い違う。要件に合わせて消すか書き換える必要がある。

### docs/todo との衝突

`docs/todo/` に、`KeyText`・Ctrl の制御文字・Alt の ESC を扱う作業ノートはなかった。
