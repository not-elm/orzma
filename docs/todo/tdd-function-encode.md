# Test cases: FunctionKey::encode

状態: 承認済み

対象は `FunctionKey::encode`（`crates/orzma_tty/src/input/keyboard/function.rs:24`、`pub fn encode(self, mods: &TerminalModifiers) -> Vec<u8>`）。参照した仕様は `xterm-ctlseqs.pdf`（PC-Style Function Keys）、`vt510.pdf`（DECFNK）、`vt220.pdf`（表 3-4）、`ECMA-48.pdf`（FNK）。citation は 26/26 件を `verify` で検証した。ケースは 6 件で、うち要確認が 5 件。API 改定の提案は 1 組（P ブロッカー 3 件）で、前回から変わっていない。

今回の実行は再実行で、前回の一覧（2026-10-22 承認）に F1〜F4 のケースを 3 件足した。前回は xterm と VT510 の食い違いのため F1〜F4 のケースを出していなかったが、作者が 2026-10-22 に会話で xterm 側（修飾キーなしは `SS3 P`〜`S`、修飾キー付きは `CSI 1 ; m P`〜`S`。要件 `docs/requirements/2026-10-21-xterm-key-encoding.md` の R1・R3）を採ると決めたので、その決定のもとで TC-03・TC-04・TC-06 を出した（付録「仕様の矛盾」）。優先度順に並べ直したため、前回の TC-03（Meta のケース）は TC-05 に番号が変わった。中身は前回と同じ。

この一覧は仕様から導いて自分で検証したもので、作者が 2026-10-22 に読み、会話で承認した。

ケースは High → Medium の順に並べている。High だけ読めば、仕様がはっきり述べている振る舞いはすべて含まれる。

## テストケース一覧

| # | 名前 | Source | 優先度 | 確認 |
| - | - | - | - | - |
| TC-01 | `f5_through_f12_without_a_modifier_send_their_tilde_sequences` | C1 | High | 自明 |
| TC-02 | `f5_through_f12_with_modifiers_append_the_modifier_code_as_a_parameter` | C1, C2, C3, C4 | High | 要確認 |
| TC-03 | `f1_through_f4_without_a_modifier_send_their_ss3_sequences` | X1, X2 | High | 要確認 |
| TC-04 | `f1_through_f4_with_modifiers_switch_from_ss3_to_csi_with_the_modifier_code` | X1, X2, C2, C3, X5, X3 | High | 要確認 |
| TC-05 | `meta_is_left_out_of_the_function_key_modifier_code` | C5, C3 | Medium | 要確認 |
| TC-06 | `meta_is_left_out_of_the_modifier_code_of_f1_through_f4` | C5, X1, X2, C3, X5 | Medium | 要確認 |

Source タグ — **C1**: xterm-ctlseqs.pdf p.47（F5〜F12 の送出列）／**C2**: p.48（修飾キーは終端文字の前に引数として付く）／**C3**: p.48（修飾値 2〜8）／**C4**: p.48（shift-F5 の例）／**C5**: p.48（alwaysUseMods のときだけ Meta を数える）／**X1**: p.47（F1〜F4 の送出列）／**X2**: p.48（修飾キーを引数で送るとき F1〜F4 の SS3 は CSI に変わる）／**X3**: p.27（修飾キー付きの F3 の例）／**X5**: p.27（修飾値は 2 番目の引数）。全文は付録の契約表にある。

6 件とも仕様由来で、`TC-A`（リポジトリの doc だけを根拠にするケース）はない。TC-03・TC-04・TC-06 は、順位の外にある xterm-ctlseqs と VT510 が食い違う F1〜F4 について、作者が xterm 側を採ると決めたことで書けるようになったケースなので、すべて要確認にした。Rust では、SS3 と CSI を 7 ビットの `ESC O` と `ESC [` で書いている（K1、K2）。

テストコードは `crates/orzma_tty/src/input/keyboard/function.rs` の `#[cfg(test)] mod tests` に追加する。TC-01・TC-02・TC-05 はすでに転記済みで、新しく転記するのは TC-03・TC-04・TC-06 の 3 件。TC-04 は既存のヘルパ `held` を使う。今の木にあるシグネチャに対して書いているのでコンパイルは通るはずだが、このスキルはビルドしていない。本体が `todo!()` なので、実装するまではテストは panic して失敗する。

## TC-01 — 修飾キーなしの F5〜F12 は、それぞれの番号の `CSI n ~` を送る

| | |
| - | - |
| Setup | `TerminalModifiers::default()` |
| Act | `FunctionKey::F5.encode(&none)` … `FunctionKey::F12.encode(&none)` |
| Expect | F5 → `ESC [ 1 5 ~` **[C1]** ／ F6 → `ESC [ 1 7 ~` **[C1]** ／ F7 → `ESC [ 1 8 ~` **[C1]** ／ F8 → `ESC [ 1 9 ~` **[C1]** ／ F9 → `ESC [ 2 0 ~` **[C1]** ／ F10 → `ESC [ 2 1 ~` **[C1]** ／ F11 → `ESC [ 2 3 ~` **[C1]** ／ F12 → `ESC [ 2 4 ~` **[C1]** |

番号が連続していない（16 と 22 が欠けている）ところが、素直な実装の間違えやすい点になる。`F5 → 15` から 1 ずつ数える実装は F6 で 16 を出すので、8 キーすべてを確かめる。修飾キーを持たない値で呼ぶので、引数を付けない形（`; 1` も付けない）であることも、この表がそのまま決めている。

```rust
/// Asserts that F5 through F12 without a modifier send `CSI n ~` with
/// each key's own number, skipping 16 and 22.
///
/// Case: the user presses F10 to quit htop, or F5 to copy a file in
/// Midnight Commander, with no modifier key held.
#[test]
fn f5_through_f12_without_a_modifier_send_their_tilde_sequences() {
    let none = TerminalModifiers::default();
    let cases: [(FunctionKey, &[u8]); 8] = [
        (FunctionKey::F5, b"\x1b[15~"),
        (FunctionKey::F6, b"\x1b[17~"),
        (FunctionKey::F7, b"\x1b[18~"),
        (FunctionKey::F8, b"\x1b[19~"),
        (FunctionKey::F9, b"\x1b[20~"),
        (FunctionKey::F10, b"\x1b[21~"),
        (FunctionKey::F11, b"\x1b[23~"),
        (FunctionKey::F12, b"\x1b[24~"),
    ];
    for (key, expected) in cases {
        assert_eq!(key.encode(&none), expected, "{key:?}");
    }
}
```

## TC-02 — 修飾キー付きの F5〜F12 は、修飾値を 2 番目の引数として `CSI n ; m ~` を送る

| | |
| - | - |
| Setup | Shift・Alt・Ctrl の 7 通りの組み合わせの `TerminalModifiers`（Meta は false） |
| Act | F5〜F12 のそれぞれを、7 通りの修飾キーで `encode` する |
| Expect | 番号 n は TC-01 と同じ **[C1]** ／ 修飾値は終端の `~` の前に `; m` として付く **[C2]** ／ m は Shift 2、Alt 3、Shift+Alt 4、Control 5、Shift+Control 6、Alt+Control 7、Shift+Alt+Control 8 **[C3]** ／ Shift+F5 は `ESC [ 1 5 ; 2 ~` **[C4]** |

C2 は条件付きの記述（修飾キーがあれば引数が付く）で、付かない側は TC-01 が受け持つ。7 通りすべてを 8 キーで回すのは、修飾値を足し算で作る実装（Shift 1、Alt 2、Ctrl 4 に 1 を足す）と、組み合わせごとの表で作る実装のどちらも、同じ結果になることを確かめるため。C4 は仕様に載っている唯一の具体例なので、ループとは別に字面どおり 1 行で確かめる。複数の契約エントリを使うので要確認にした。

```rust
/// Asserts that F5 through F12 held with Shift, Alt, Control, or any
/// combination of them send `CSI n ; m ~`, where `m` is the xterm
/// modifier code from 2 to 8.
///
/// Case: the user presses Shift-F5, Ctrl-F7, or Ctrl-Alt-F12 in a
/// full-screen editor that binds the modified function keys.
#[test]
fn f5_through_f12_with_modifiers_append_the_modifier_code_as_a_parameter() {
    let keys: [(FunctionKey, u8); 8] = [
        (FunctionKey::F5, 15),
        (FunctionKey::F6, 17),
        (FunctionKey::F7, 18),
        (FunctionKey::F8, 19),
        (FunctionKey::F9, 20),
        (FunctionKey::F10, 21),
        (FunctionKey::F11, 23),
        (FunctionKey::F12, 24),
    ];
    let modifiers: [(TerminalModifiers, u8); 7] = [
        (held(true, false, false), 2),
        (held(false, true, false), 3),
        (held(true, true, false), 4),
        (held(false, false, true), 5),
        (held(true, false, true), 6),
        (held(false, true, true), 7),
        (held(true, true, true), 8),
    ];
    for (key, number) in keys {
        for (mods, code) in modifiers {
            let expected = format!("\x1b[{number};{code}~");
            assert_eq!(
                key.encode(&mods),
                expected.as_bytes(),
                "{key:?} with {mods:?}"
            );
        }
    }
    assert_eq!(
        FunctionKey::F5.encode(&held(true, false, false)),
        b"\x1b[15;2~"
    );
}
```

テスト用のヘルパ（`mod tests` の中に置く。転記済み）:

```rust
fn held(shift: bool, alt: bool, ctrl: bool) -> TerminalModifiers {
    TerminalModifiers {
        shift,
        alt,
        ctrl,
        meta: false,
    }
}
```

## TC-03 — 修飾キーなしの F1〜F4 は `SS3 P`〜`SS3 S` を送る

| | |
| - | - |
| Setup | `TerminalModifiers::default()` |
| Act | `FunctionKey::F1.encode(&none)` … `FunctionKey::F4.encode(&none)` |
| Expect | F1 → `ESC O P` **[X1]** ／ F2 → `ESC O Q` **[X1]** ／ F3 → `ESC O R` **[X1]** ／ F4 → `ESC O S` **[X1]** ／ 修飾キーがないので SS3 は CSI に変わらない **[X2]** |

X1 の表そのままのケースだが、X2 が条件付きの記述（修飾キーを引数で送るときだけ SS3 を CSI に変える）なので、その「満たさない側」もこのケースが受け持つ。防ぎたいのは、F5〜F12 と同じ `CSI n ~` の形に揃えてしまう実装（VT510 の DECFNK や、xterm の oldXtermFKeys の `CSI 1 1 ~` 系、X6）と、修飾キーの有無にかかわらず `CSI P` を出す実装の 2 つ。作者の決定で初めて書けるようになったケースなので、形は単純でも要確認にした。

```rust
/// Asserts that F1 through F4 without a modifier send `SS3 P` through
/// `SS3 S`.
///
/// Case: the user presses F1 to open the help screen of htop or Midnight
/// Commander, with no modifier key held.
#[test]
fn f1_through_f4_without_a_modifier_send_their_ss3_sequences() {
    let none = TerminalModifiers::default();
    let cases: [(FunctionKey, &[u8]); 4] = [
        (FunctionKey::F1, b"\x1bOP"),
        (FunctionKey::F2, b"\x1bOQ"),
        (FunctionKey::F3, b"\x1bOR"),
        (FunctionKey::F4, b"\x1bOS"),
    ];
    for (key, expected) in cases {
        assert_eq!(key.encode(&none), expected, "{key:?}");
    }
}
```

## TC-04 — 修飾キー付きの F1〜F4 は、SS3 を CSI に変え、`CSI 1 ; m P`〜`S` を送る

| | |
| - | - |
| Setup | Shift・Alt・Ctrl の 7 通りの組み合わせの `TerminalModifiers`（Meta は false） |
| Act | F1〜F4 のそれぞれを、7 通りの修飾キーで `encode` する |
| Expect | 終端文字は X1 と同じ P・Q・R・S **[X1]** ／ 先頭の SS3 は CSI に変わる **[X2]** ／ 修飾値は終端文字の前に引数として付く **[C2]** ／ 修飾値は 2 番目の引数で、1 番目の引数は `1` **[X5]** ／ m は Shift 2 〜 Shift+Alt+Control 8 **[C3]** ／ Shift+F3 は `ESC [ 1 ; 2 R`、Control+F3 は `ESC [ 1 ; 5 R`、Shift+Control+F3 は `ESC [ 1 ; 6 R` **[X3]** |

X2 の条件を満たす側。1 番目の引数 `1` をはっきり示しているのは、F3 の例（X3）と「2 番目の引数が修飾値を表す」という一文（X5）だけで、F1・F2・F4 については X5 を同じように当てはめている。要件の R3 も 4 キーとも `CSI 1 ; m` と決めている。防ぎたいのは、`1 ;` を落として `CSI 2 P` を出す実装と、SS3 のまま引数を付けて `ESC O 2 P` のような列を出す実装。X3 の 3 つの例は仕様に載っている具体例なので、ループとは別に字面どおりに確かめる。複数の契約エントリを使い、作者の決定にも依っているので要確認にした。

```rust
/// Asserts that F1 through F4 held with Shift, Alt, Control, or any
/// combination of them switch from `SS3` to `CSI 1 ; m` with the same
/// final character, where `m` is the xterm modifier code from 2 to 8.
///
/// Case: the user presses Shift-F1, Ctrl-F3, or Ctrl-Alt-F4 in a
/// full-screen editor that binds the modified function keys.
#[test]
fn f1_through_f4_with_modifiers_switch_from_ss3_to_csi_with_the_modifier_code() {
    let keys: [(FunctionKey, char); 4] = [
        (FunctionKey::F1, 'P'),
        (FunctionKey::F2, 'Q'),
        (FunctionKey::F3, 'R'),
        (FunctionKey::F4, 'S'),
    ];
    let modifiers: [(TerminalModifiers, u8); 7] = [
        (held(true, false, false), 2),
        (held(false, true, false), 3),
        (held(true, true, false), 4),
        (held(false, false, true), 5),
        (held(true, false, true), 6),
        (held(false, true, true), 7),
        (held(true, true, true), 8),
    ];
    for (key, final_byte) in keys {
        for (mods, code) in modifiers {
            let expected = format!("\x1b[1;{code}{final_byte}");
            assert_eq!(
                key.encode(&mods),
                expected.as_bytes(),
                "{key:?} with {mods:?}"
            );
        }
    }
    assert_eq!(
        FunctionKey::F3.encode(&held(true, false, false)),
        b"\x1b[1;2R"
    );
    assert_eq!(
        FunctionKey::F3.encode(&held(false, false, true)),
        b"\x1b[1;5R"
    );
    assert_eq!(
        FunctionKey::F3.encode(&held(true, false, true)),
        b"\x1b[1;6R"
    );
}
```

## TC-05 — Meta は修飾値に数えない（F5〜F12）

前回の TC-03。中身は変えていない。

| | |
| - | - |
| Setup | `TerminalModifiers { meta: true, .. }` を、ほかの修飾キーなし・Shift と一緒・Shift+Alt+Ctrl と一緒の 3 通りで用意する |
| Act | `FunctionKey::F5.encode(&meta)`、`FunctionKey::F5.encode(&meta_shift)`、`FunctionKey::F12.encode(&meta_all)` |
| Expect | Meta だけの F5 は `ESC [ 1 5 ~` で、引数を付けない **[C5]** ／ Meta+Shift の F5 は `ESC [ 1 5 ; 2 ~` **[C5][C3]** ／ Meta+Shift+Alt+Ctrl の F12 は `ESC [ 2 4 ; 8 ~` **[C5][C3]** |

C5 は「alwaysUseMods を設定したときだけ Meta も数え、9〜16 になる」と述べているので、モードごとに 1 件ずつケースを出す形になる。orzma には alwaysUseMods にあたる入力がないので、設定しない側だけを書いた（設定する側は付録「API 改定」の B1）。「数えない」ときに Meta だけが押された F5 が引数なしの形になるのは、C5 の条件の裏から読んだものなので、優先度を Medium、確認を要確認にした。VT510 の DECFNK の修飾値の表（C9）も 2〜8 しか持たず、Meta はない。防ぎたいのは、`meta` を 8 として足してしまう実装（Meta+Shift の F5 で `; 10` を出す）。キーボードからの経路では Cmd・Super 付きのキーはエンコーダーに届く前に捨てられるので、このケースが守るのは `encode` そのものの約束になる。

```rust
/// Asserts that Meta is left out of the function key modifier code, so
/// Meta alone adds no parameter and Meta with other modifiers sends the
/// code those modifiers give.
///
/// Case: a caller hands the encoder a function key with the Meta flag
/// set, as a press of F5 or F12 with the Super key held would carry.
#[test]
fn meta_is_left_out_of_the_function_key_modifier_code() {
    let meta = TerminalModifiers {
        meta: true,
        ..Default::default()
    };
    assert_eq!(FunctionKey::F5.encode(&meta), b"\x1b[15~");
    let meta_shift = TerminalModifiers {
        meta: true,
        shift: true,
        ..Default::default()
    };
    assert_eq!(FunctionKey::F5.encode(&meta_shift), b"\x1b[15;2~");
    let meta_all = TerminalModifiers {
        meta: true,
        shift: true,
        alt: true,
        ctrl: true,
    };
    assert_eq!(FunctionKey::F12.encode(&meta_all), b"\x1b[24;8~");
}
```

## TC-06 — Meta は修飾値に数えない（F1〜F4）

| | |
| - | - |
| Setup | TC-05 と同じ 3 通りの `TerminalModifiers`（Meta だけ、Meta+Shift、Meta+Shift+Alt+Ctrl） |
| Act | `FunctionKey::F1.encode(&meta)`、`FunctionKey::F1.encode(&meta_shift)`、`FunctionKey::F4.encode(&meta_all)` |
| Expect | Meta だけの F1 は `ESC O P` で、SS3 は CSI に変わらない **[C5][X1][X2]** ／ Meta+Shift の F1 は `ESC [ 1 ; 2 P` **[C5][X2][X5][C3]** ／ Meta+Shift+Alt+Ctrl の F4 は `ESC [ 1 ; 8 S` **[C5][X2][X5][C3]** |

TC-05 と同じ C5 の「設定しない側」だが、F1〜F4 では修飾キーがあるかどうかで SS3 と CSI が切り替わる（X2）ので、F5〜F12 とは別の分かれ道になる。防ぎたいのは、「何かの修飾フラグが立っているか」を `meta` も含めて判定し、Meta だけの F1 で CSI の形（`ESC [ 1 ; 1 P` や `ESC [ P`）に切り替えてしまう実装。TC-05 と同じ理由で、優先度を Medium、確認を要確認にした。

```rust
/// Asserts that Meta is left out of the modifier code of F1 through F4,
/// so Meta alone keeps the `SS3` form and Meta with other modifiers sends
/// the `CSI` form those modifiers give.
///
/// Case: a caller hands the encoder F1 or F4 with the Meta flag set, as a
/// press with the Super key held would carry.
#[test]
fn meta_is_left_out_of_the_modifier_code_of_f1_through_f4() {
    let meta = TerminalModifiers {
        meta: true,
        ..Default::default()
    };
    assert_eq!(FunctionKey::F1.encode(&meta), b"\x1bOP");
    let meta_shift = TerminalModifiers {
        meta: true,
        shift: true,
        ..Default::default()
    };
    assert_eq!(FunctionKey::F1.encode(&meta_shift), b"\x1b[1;2P");
    let meta_all = TerminalModifiers {
        meta: true,
        shift: true,
        alt: true,
        ctrl: true,
    };
    assert_eq!(FunctionKey::F4.encode(&meta_all), b"\x1b[1;8S");
}
```

## 付録

### 契約表

`pdftotext -layout` で抽出したテキストの行番号。ページは PDF のページ番号を計算したもの。今回の抽出には Git for Windows に入っている xpdf の `pdftotext` 4.00 を使ったので、前回の文書とは行番号が違う。行番号はこのセッションでの目安でしかない。表の行が 10 行を超えるエントリは、`verify` の幅の上限に合わせて範囲を 2 つに分け、両方を検証した。

| ID | Governs | Shape | Statement | Citation |
| --- | --- | --- | --- | --- |
| C1 | PC-Style Function Keys | unconditional | F5 CSI 1 5 ~, F6 CSI 1 7 ~, F7 CSI 1 8 ~, F8 CSI 1 9 ~ ／ F9 CSI 2 0 ~, F10 CSI 2 1 ~, F11 CSI 2 3 ~, F12 CSI 2 4 ~ | xterm-ctlseqs.pdf p.47, L2751-2757 ／ L2759-2765 |
| C2 | PC-Style Function Keys | conditional | "In normal mode, i.e., a Sun/PC keyboard when the sunKeyboard resource is false (and none of the other keyboard resources such as oldXtermFKeys resource is set), xterm encodes function key modifiers as parameters appended before the final character of the control sequence." | xterm-ctlseqs.pdf p.48, L2789-2792 |
| C3 | PC-Style Function Keys | multi-function | 2 Shift, 3 Alt, 4 Shift + Alt, 5 Control, 6 Shift + Control ／ 7 Alt + Control, 8 Shift + Alt + Control | xterm-ctlseqs.pdf p.48, L2800-2808 ／ L2810-2812 |
| C4 | PC-Style Function Keys | unconditional | "For example, shift-F5 would be sent as CSI 1 5 ; 2 ~" | xterm-ctlseqs.pdf p.48, L2832 |
| C5 | PC-Style Function Keys | mode-dependent | "If the alwaysUseMods resource is set, the Meta modifier also is recognized, making parameters 9 through 16." | xterm-ctlseqs.pdf p.48, L2834-2835 |
| C13 | PC-Style Function Keys | multi-function | 9 Meta, 10 Meta + Shift, 11 Meta + Alt, 12 Meta + Alt + Shift, 13 Meta + Ctrl ／ 14 Meta + Ctrl + Shift, 15 Meta + Ctrl + Alt, 16 Meta + Ctrl + Alt + Shift（B1 の提案でだけ使う） | xterm-ctlseqs.pdf p.48, L2814-2822 ／ L2824-2828 |
| X1 | PC-Style Function Keys | unconditional | F1 SS3 P, F2 SS3 Q, F3 SS3 R, F4 SS3 S | xterm-ctlseqs.pdf p.47, L2743-2749 |
| X2 | PC-Style Function Keys | conditional | "As a special case, the SS3 sent before F1 through F4 is altered to CSI when sending a function key modifier as a parameter." | xterm-ctlseqs.pdf p.48, L2792-2794 |
| X3 | DSR（の注記） | unconditional | "Note: it is possible for this sequence to be sent by a function key. For example, with the default keyboard configuration the shifted F3 key may send (with shift-, control-, alt-modifiers) CSI 1 ; 2 R , or CSI 1 ; 5 R , or CSI 1 ; 6 R" | xterm-ctlseqs.pdf p.27, L1522-1529 |
| X5 | DSR（の注記） | unconditional | "The second parameter encodes the modifiers; values range from 2 to 16." | xterm-ctlseqs.pdf p.27, L1531-1532 |
| X4 | PC-Style Function Keys | conditional | "Older versions of xterm implement different escape sequences for F1 through F4, with a CSI prefix. These can be activated by setting the oldXtermFKeys resource." | xterm-ctlseqs.pdf p.47, L2770-2772 |
| X6 | PC-Style Function Keys | unconditional | F1 CSI 1 1 ~, F2 CSI 1 2 ~, F3 CSI 1 3 ~, F4 CSI 1 4 ~（oldXtermFKeys の形） | xterm-ctlseqs.pdf p.47, L2780-2786 |
| X7 | PC-Style Function Keys | unconditional | "However, since they do not correspond to any hardware terminal, they have been deprecated." | xterm-ctlseqs.pdf p.47, L2772-2773 |
| K1 | SS3 | unconditional | "ESC O Single Shift Select of G3 Character Set (SS3 is 0x8f), VT220." | xterm-ctlseqs.pdf p.5, L253-254 |
| K2 | CSI | unconditional | "ESC [ Control Sequence Introducer (CSI is 0x9b)." | xterm-ctlseqs.pdf p.6, L271-272 |
| V1 | DECFNK | unconditional | "The DECFNK sequences encode the top row function keys, the six editing keypad keys, and the four cursor keys. The top row function keys, ( F1 through F20 ), can be encoded unmodified or in combination with Shift , Ctrl , and Alt key combinations." | vt510.pdf p.164, L7446-7448 |
| C7 | DECFNK | unconditional | Format: CSI Ps1 ; Ps2 ~ | vt510.pdf p.164, L7456 |
| C8 | DECFNK | unconditional | Ps2: "0 or omitted none" | vt510.pdf p.165, L7535 |
| C9 | DECFNK | multi-function | Ps2: 2 Shift, 3 Alt, 4 Alt + Shift, 5 Control, 6 Control + Shift ／ 7 Alt + Control, 8 Alt + Control + Shift（C3 と一致） | vt510.pdf p.165, L7539-7547 ／ p.166, L7550-7552 |
| C12 | DECFNK | conditional | "Top row function keys F1 through F5 are only available when they are not used for local terminal functions." | vt510.pdf p.166, L7565-7566 |
| V2 | VT100 mode | unconditional | "Function keys also work in VT100 mode." | vt510.pdf p.43, L1992 |
| C10 | Top Row Function Keys | mode-dependent | F11 (ESC): VT200 mode CSI 2 3 ~, VT100/VT52 modes ESC ／ F12 (BS): VT200 mode CSI 2 4 ~, VT100/VT52 modes BS | vt220.pdf p.74, L4716-4718 ／ L4720-4721 |
| C11 | Top Row Function Keys | mode-dependent | 表の列見出し "VT200 Mode" と "VT100, VT52 Modes" | vt220.pdf p.74, L4684 |
| T1 | Top Row Function Keys | unconditional | "The first five keys (F1 through F5) labeled Hold Screen, Print Screen, Set-Up, Data/Talk, and Break, do not send codes; they are local function keys." | vt220.pdf p.18, L791-792 |
| E1 | FNK | unconditional | "FNK - FUNCTION KEY Notation: (Pn) Representation: CSI Pn 02/00 05/07" | ECMA-48.pdf p.58, L3651-3653 |
| E2 | FNK | unconditional | "FNK is a control function in which the parameter value identifies the function key which has been operated." | ECMA-48.pdf p.58, L3657-3658 |

citations verified: 26/26（範囲を分けたものも含め、範囲 31 個すべて）。前回 V1 として記録した「Ps1: 11 F1, 12 F2, 13 F3, 14 F4」と、C6 として記録した「Ps1: 15 F5 … 24 F12」は、今回の抽出では棄却された（下の「仕様にないもの」）。V1 は、同じ節の別の記述（DECFNK が F1〜F20 を符号化するという文）で記録し直し、再検証を通した。C6 は、どのケースのタグにも使っていない裏付けだったので、記録し直さずに落とした。

手順の中での順位づけ:

- **F5**：VT220 は F5 をローカル機能でコードを送らないとしている（T1）。一方 VT510 は、ローカル機能に使われていなければ F5 を DECFNK として送る（V1、C12）。順位が上の VT510 を採った。
- **FNK と DECFNK**：ECMA-48 の FNK（`CSI Pn SP W`、E1・E2）と VT510 の DECFNK（`CSI Ps1 ; Ps2 ~`、C7）は形が違う。順位が上の VT510 を採った。
- **F1〜F4**：順位の外にある xterm-ctlseqs と VT510 が食い違う。作者の決定で xterm 側を採った（「仕様の矛盾」）。C12 のうち F1〜F4 にかかる部分は、この決定で VT510 側を採らなかったので、F5 にだけ残る（B3）。

### 探索した語

| 語 | レベル | 結果 |
| --- | --- | --- |
| function key | 1（メソッドの doc「Encodes the function key to the pty input buffer.」） | xterm-ctlseqs の PC-Style 節、vt510 の DECFNK、vt220 の 3.2.4 と表 3-4、ECMA-48 の FNK |
| top-row function key / F1 / F5 / F11 / F12 | 2・3（enum の doc「A top-row function key, F1 through F12.」と `//!`「The top-row function keys F1 to F12.」） | 同上 |
| DECFNK | function key の検索で出た語 | vt510 p.164-166 の定義 |
| alwaysUseMods / Meta | function key の検索で出た語 | xterm-ctlseqs p.48（C5、C13） |
| SS3 P | F1〜F4 の送出列の確認 | xterm-ctlseqs p.47 の表（X1）。テンキーの PF1〜PF4 の表にも同じ `SS3 P` があるが、別のキー（下の棄却） |
| oldXtermFKeys | F1〜F4 の別の形の確認 | xterm-ctlseqs p.47（X4、X6、X7）、p.48（C2 の条件の中） |
| CSI 1 ; / second parameter | F1〜F4 の修飾形の確認 | xterm-ctlseqs p.27 の DSR 注記（X3、X5） |
| modifyFunctionKeys | X3 の注記に出た語 | xterm-ctlseqs p.27 の `CSI > Ps n`（修飾キーを引数でなく別の列で表す設定。orzma に該当する入力はないので契約に入れていない） |
| ESC O / ESC [ | SS3・CSI の 7 ビットの形 | xterm-ctlseqs p.5-6（K1、K2） |
| FNK | ECMA-48 の function key | ECMA-48 8.3.52（E1、E2） |
| Function keys also work in VT100 mode | VT100 モードで VT510 が黙っているかの確認 | vt510 L1992（V2）だけで、送るコードは書かれていない。そのため VT220 の C10 が効く |
| local function | C12・T1 の確認 | vt510 L7565、vt220 L791 |

level 1 の語で振る舞いの記述に届いたので、level 4（型名 `TerminalModifiers` の展開）には降りていない。修飾キーの語（Shift、Alt、Control、Meta）は、level 1 で届いた節の中で見つかった。

定義でない行として棄却したもの（確認できたもの）:

- xterm-ctlseqs：L47（目次）、L1532（相互参照「See the section PC-Style Function Keys」）、L2692-2698（テンキーの PF1〜PF4 で、上段の F1〜F4 ではない）、L2882-2888（VT220-Style の節のテンキーの PF1〜PF4）
- vt510：L239（目次）、L642（表の目次）、L4814（制御機能の要約表）、L13890（相互参照）
- vt220：L4779、L4817（ASCII の符号表からの相互参照）
- ECMA-48：L484（目次）、L1399（符号表）、L2881（一覧表）

### 仕様にないもの

- 修飾キー付きの F1・F2・F4 の 1 番目の引数 `1`。`1 ;` をはっきり示しているのは F3 の例（X3）だけだった。TC-04・TC-06 では、「2 番目の引数が修飾値」という X5 を F1・F2・F4 にも同じように当てはめ、作者の決定（R3）で 4 キーを揃えた。
- 前回の V1「Ps1: 11 F1, 12 F2, 13 F3, 14 F4」（vt510.pdf p.165, L7482-7491 で照合）：REJECTED（1/8 matched）。今回の抽出では DECFNK の表の Ps1 の列とキー名の列が別々の行に分かれていて、番号とキー名が並ばない。同じ節の別の文で V1 を記録し直した（契約表）。
- 前回の C6「Ps1: 15 F5, 17 F6, 18 F7, 19 F8, 20 F9, 21 F10, 23 F11, 24 F12」（vt510.pdf p.165, L7486-7495 で照合）：REJECTED（1/16 matched）。理由は上と同じ。どのケースのタグにも使っていないので落とした。

### 仕様の矛盾

**F1〜F4 の送出列**（作者が 2026-10-22 に xterm 側を採ると決めた）

| 説明書 | 修飾キーなし | 修飾キー付き | 出典 |
| --- | --- | --- | --- |
| xterm-ctlseqs（PC-Style） | `SS3 P`、`SS3 Q`、`SS3 R`、`SS3 S` | SS3 が CSI に変わり、引数が付く。F3 の例は `CSI 1 ; 2 R` | X1、X2、X3、X5 |
| vt510（DECFNK） | DECFNK の形 `CSI Ps1 ; Ps2 ~` | 同じ形で Ps2 に修飾値 | V1、C7、C8 |
| vt220（表 3-4） | ローカル機能で、コードを送らない | — | T1（VT510 より順位が下なので、VT510 に負ける） |

xterm-ctlseqs は順位の外にある説明書なので、VT510 との食い違いはこのスキルでは解消しない。xterm-ctlseqs 自身も、`CSI 1 1 ~` 系（X6）は oldXtermFKeys で選べる古い形で、どのハードウェア端末にも対応しないため非推奨だと書いている（X4、X7）。作者は要件 `docs/requirements/2026-10-21-xterm-key-encoding.md` の R1・R3 で xterm 側を選び、2026-10-22 にこの会話で、その前提で F1〜F4 のケースを出すよう求めた。TC-03・TC-04・TC-06 はこの決定のもとで出したもので、VT510 の DECFNK の形（`CSI 11 ~` など）のケースは出していない。

### API 改定

前回の一覧の承認のあとも、シグネチャは `pub fn encode(self, mods: &TerminalModifiers) -> Vec<u8>` のままなので、今回も今のシグネチャで書いた。下の 3 件は前回と同じ提案で、1 組の変更として扱う。どれも P（パラメータがない）のブロッカー。今のシグネチャのままにすると、C5 の「設定する側」、C10 の VT100/VT52 モードの側、C12 の「ローカル機能に使われている側」からはケースが出ない。採るかどうかは引き続き作者の判断。

**B1 — alwaysUseMods を設定した状態を作れない（C5、C13）**

- 書けない行：Setup「alwaysUseMods を設定した状態」。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, always_use_mods: bool) -> Vec<u8>`。呼び出し側の `encode_key` にも、この値をどこかから渡す必要がある（今の `VtModes` にも設定にも、該当するものはない）。
- 影響するケース：
  - 今のシグネチャの TC-05・TC-06：書ける（設定しない側）。
  - 提案のもとでの TC-05・TC-06：Setup に `always_use_mods = false` を加えるだけで、Expect は同じ。
  - 提案のもとでの新しいケース（番号なし）：Setup `always_use_mods = true`、`meta: true` ／ Act `FunctionKey::F5.encode(&meta, true)`、`FunctionKey::F1.encode(&meta, true)` ／ Expect F5 は `ESC [ 1 5 ; 9 ~` **[C5][C13]**、Meta+Shift の F5 は `ESC [ 1 5 ; 1 0 ~` **[C5][C13]**、F1 は `ESC [ 1 ; 9 P` **[C5][C13][X2][X5]**。
- 補足：要件ドキュメントは、Cmd・Super 付きのキーを今どおり捨てる（Meta を数えない）と決めている。

**B2 — VT100/VT52 モードを作れない（C10、C11）**

- 書けない行：Setup「端末が VT100 モードまたは VT52 モードで動いている」。orzma には、動作レベルや VT52 モードにあたる型が見当たらなかった（`orzma_vt` と `orzma_tty` で `vt52`、`DECANM`、`DECSCL`、`conformance` を検索した）。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, level: OperatingLevel) -> Vec<u8>`。新しい型 `OperatingLevel`（VT200 以上か、VT100/VT52 か）と、それを VT の状態から `encode_key` へ渡す経路が要る。
- 影響するケース：
  - 今のシグネチャ：VT100/VT52 モードの側は書けない。TC-01 は VT200 モードの側として、そのまま成り立つ。
  - 提案のもとでの新しいケース（番号なし）：Setup VT100 モード ／ Act `FunctionKey::F11.encode(&none, level)`、`FunctionKey::F12.encode(&none, level)` ／ Expect F11 は `ESC`（0x1b）**[C10]**、F12 は `BS`（0x08）**[C10]**。

**B3 — F5 がローカル機能に使われている状態を作れない（C12）**

- 書けない行：Setup「F5 がローカル機能に割り当てられている」。Expect「F5 は何も送らない」。今の戻り値 `Vec<u8>` では「送らない」を空の列でしか表せず、ローカル機能かどうかを渡す入力もない。C12 は F1〜F5 について述べているが、F1〜F4 は作者の決定で xterm 側を採ったので、ここでは F5 だけを扱う。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, local: bool) -> Option<Vec<u8>>`。呼び出し側の `encode_key` の戻り値の扱いも変わる。
- 影響するケース：
  - 今のシグネチャ：ローカル機能の側は書けない。TC-01・TC-02 は、ローカル機能に使われていない側として成り立つ。
  - 提案のもとでの新しいケース（番号なし）：Setup `local = true` ／ Act `FunctionKey::F5.encode(&none, true)` ／ Expect `None` **[C12]**。

### 出典なしの改善提案

- 修飾値 m（Shift・Alt・Ctrl から 2〜8 を作る値）の計算は、要件の R5・R6（修飾キー付きの矢印・Home・End・Insert・Delete・PageUp・PageDown）でも使う。`FunctionKey` の中ではなく、両方から呼べる場所に置くと重複しない。これは仕様が求めることではなく、ケースにも影響しない。
- TC-02 と TC-04 は、7 通りの修飾キーと修飾値の組を同じ配列で持っている。`mod tests` の中で `held` と並ぶヘルパにまとめると、2 か所を揃えて直さずに済む。ケースには影響しない。
