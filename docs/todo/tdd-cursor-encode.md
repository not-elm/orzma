# Test cases: CursorKey::encode

状態: 承認済み

対象は `CursorKey::encode`（`crates/orzma_tty/src/input/keyboard/cursor.rs:29`、`pub fn encode(self, mods: &TerminalModifiers, app_cursor_keys: bool) -> Vec<u8>`）。参照した仕様は `xterm-ctlseqs.pdf`（PC-Style Function Keys、VT52-Style Function Keys）、`vt220.pdf`（4.6.8 DECCKM、表 3-2）、`vt510.pdf`（DECCKM、DECFNK）。citation は 20/20 件（説明書 18 件、リポジトリの doc コメント 2 件）を検証した。ケースは 5 件で、すべて要確認。API 改定の提案は 1 組（P ブロッカー 2 件）。

作者は 2026-10-22 にこの会話で、次の 2 つを前提として渡した。

- 修飾キー付きのカーソルキーで xterm と VT510 が食い違う点（VT510 の DECFNK はカーソルキーに Alt しか使わない）は、要件 `docs/requirements/2026-10-21-xterm-key-encoding.md` の R5 どおり xterm 側を採る。
- 修飾キー付きならアプリケーションモードでも CSI にするという挙動の出典として、xterm(1) を `docs/references/` に足さない。その挙動はメソッドの doc コメントを出典（kind 2）にしてよい。

この一覧は仕様から導いて自分で検証したもので、作者が 2026-10-22 に読み、会話で承認した。

ケースは High → Medium の順に並べている。High だけ読めば、仕様がはっきり述べている振る舞いはすべて含まれる。

## テストケース一覧

| # | 名前 | Source | 優先度 | 確認 |
| - | - | - | - | - |
| TC-01 | `cursor_keys_without_a_modifier_send_csi_sequences_in_normal_mode` | A1, A2, A3, A4, D1 | High | 要確認 |
| TC-02 | `cursor_keys_without_a_modifier_send_ss3_sequences_in_application_mode` | A1, A2, A3, A4, D1 | High | 要確認 |
| TC-03 | `cursor_keys_with_modifiers_send_csi_1_with_the_modifier_code_in_normal_mode` | CM, C2, C3, MD | Medium | 要確認 |
| TC-A1 | `cursor_keys_with_modifiers_keep_the_csi_form_in_application_mode` | MD | Medium | 要確認 |
| TC-04 | `meta_is_left_out_of_the_cursor_key_modifier_code` | C5, MM, A2, MD | Medium | 要確認 |

Source タグ — **A1**: xterm-ctlseqs.pdf p.46（カーソルキーは DECCKM で送る列が変わる）／**A2**: p.46（矢印の Normal・Application の表）／**A3**: p.46（Home・End もカーソルキーで、DECCKM に従う）／**A4**: p.46（Home・End の表）／**D1**: vt220.pdf p.72 表 3-2（カーソルキーが送る列）／**CM**: xterm-ctlseqs.pdf p.48（修飾値 2〜8 はカーソルキーにも使う）／**C2**: p.48（修飾値は終端文字の前に引数として付く）／**C3**: p.48（修飾値 2〜8）／**C5**: p.48（alwaysUseMods のときだけ Meta を数える）／**MD**・**MM**: `CursorKey::encode` の doc コメント（下の台帳）。全文は付録の契約表にある。

TC-01・TC-02 は仕様がそのまま述べているケース。TC-03・TC-04 は、修飾値の値と付き方を仕様から取り、`CSI 1 ;` という形を doc コメント（MD）から取っている。TC-A1 は、アプリケーションモードでも CSI にするという、`docs/references/` のどの説明書も述べていない挙動を守るケースで、doc コメントだけを出典にしているので `TC-A` の番号にした。修飾キー付きの 3 件は、作者が xterm 側を採ると決めたことで書けるようになったケースでもある。TC-01・TC-02 は DECCKM の 2 つのモードを 1 件ずつ受け持つ mode-dependent の形なので、仕様がはっきり述べていても「自明」の条件（unconditional）に当たらず、要確認にした。Rust では、SS3 と CSI を 7 ビットの `ESC O` と `ESC [` で書いている（K1、K2）。

テストコードは `crates/orzma_tty/src/input/keyboard/cursor.rs` に新しく `#[cfg(test)] mod tests` を作って追加する。このファイルにはまだテストモジュールがない。`keyboard.rs` の `mod tests` にある矢印・Home・End のテストが試しているのは `encode_key` で、`CursorKey` そのものではないので、そちらには置かない。ヘルパ `held` と `modifier_codes` は、この `mod tests` の中に新しく置く。今の木にあるシグネチャに対して書いているのでコンパイルは通るはずだが、このスキルはビルドしていない。本体が `todo!()` なので、実装するまではテストは panic して失敗する。

### kind 2 の台帳

```
MD — crates/orzma_tty/src/input/keyboard/cursor.rs:22-24, CursorKey::encode
     "With any of them held, the keys send `CSI 1 ; m A` through `CSI 1 ; m F` whatever `app_cursor_keys` says, where `m` is 1 plus Shift 1, Alt 2, and Ctrl 4."
     justifies: 修飾キー付きの形が `CSI 1 ; m X` であること、DECCKM が有効でもその形のままであること
MM — crates/orzma_tty/src/input/keyboard/cursor.rs:24, CursorKey::encode
     "Meta does not count toward `m`."
     justifies: Meta だけでは修飾キーなしの形のままで、ほかの修飾キーと一緒なら Meta を除いた m になること
```

## TC-01 — DECCKM がリセットのとき、修飾キーなしのカーソルキーは CSI の列を送る

| | |
| - | - |
| Setup | `TerminalModifiers::default()`、`app_cursor_keys = false` |
| Act | `CursorKey::Up.encode(&none, false)` … `CursorKey::End.encode(&none, false)` |
| Expect | Up → `ESC [ A`、Down → `ESC [ B`、Right → `ESC [ C`、Left → `ESC [ D` **[A2][D1]** ／ Home → `ESC [ H`、End → `ESC [ F` **[A3][A4]** ／ DECCKM がリセットなので Normal の列を使う **[A1]** |

A1 が mode-dependent の記述なので、モードごとに 1 件ずつ出す。こちらはリセット側。矢印の 4 本は VT220 の表 3-2（D1）と xterm の表（A2）が同じ列を定めていて、Home・End は xterm だけが定めている（A3、A4）。VT510 は DECCKM の定義（V3）で「ANSI cursor sequences」とだけ述べ、具体的な列を書いていないので、順位が次の VT220 の表が効く。防ぎたいのは、Home・End を VT220 の編集キーの形（`CSI 1 ~`・`CSI 4 ~`）で送る実装と、矢印の終端文字を取り違える実装。

```rust
/// Asserts that the cursor keys without a modifier send `CSI A` through
/// `CSI D`, `CSI H`, and `CSI F` when DECCKM is reset.
///
/// Case: the user moves through a shell's command line with the arrow
/// keys, Home, and End, with no modifier key held.
#[test]
fn cursor_keys_without_a_modifier_send_csi_sequences_in_normal_mode() {
    let none = TerminalModifiers::default();
    let cases: [(CursorKey, &[u8]); 6] = [
        (CursorKey::Up, b"\x1b[A"),
        (CursorKey::Down, b"\x1b[B"),
        (CursorKey::Right, b"\x1b[C"),
        (CursorKey::Left, b"\x1b[D"),
        (CursorKey::Home, b"\x1b[H"),
        (CursorKey::End, b"\x1b[F"),
    ];
    for (key, expected) in cases {
        assert_eq!(key.encode(&none, false), expected, "{key:?}");
    }
}
```

## TC-02 — DECCKM がセットのとき、修飾キーなしのカーソルキーは SS3 の列を送る

| | |
| - | - |
| Setup | `TerminalModifiers::default()`、`app_cursor_keys = true` |
| Act | `CursorKey::Up.encode(&none, true)` … `CursorKey::End.encode(&none, true)` |
| Expect | Up → `ESC O A`、Down → `ESC O B`、Right → `ESC O C`、Left → `ESC O D` **[A2][D1]** ／ Home → `ESC O H`、End → `ESC O F` **[A3][A4]** ／ DECCKM がセットなので Application の列を使う **[A1]** |

A1 のセット側。Home・End も DECCKM に従う（A3）ので、矢印と同じく SS3 に変わることを確かめる。防ぎたいのは、DECCKM を矢印だけに効かせて Home・End を CSI のまま送る実装。

```rust
/// Asserts that the cursor keys without a modifier send `SS3 A` through
/// `SS3 D`, `SS3 H`, and `SS3 F` when DECCKM is set.
///
/// Case: a full-screen editor turns on application cursor keys, and the
/// user moves through the buffer with the arrow keys, Home, and End.
#[test]
fn cursor_keys_without_a_modifier_send_ss3_sequences_in_application_mode() {
    let none = TerminalModifiers::default();
    let cases: [(CursorKey, &[u8]); 6] = [
        (CursorKey::Up, b"\x1bOA"),
        (CursorKey::Down, b"\x1bOB"),
        (CursorKey::Right, b"\x1bOC"),
        (CursorKey::Left, b"\x1bOD"),
        (CursorKey::Home, b"\x1bOH"),
        (CursorKey::End, b"\x1bOF"),
    ];
    for (key, expected) in cases {
        assert_eq!(key.encode(&none, true), expected, "{key:?}");
    }
}
```

## TC-03 — DECCKM がリセットのとき、修飾キー付きのカーソルキーは `CSI 1 ; m X` を送る

| | |
| - | - |
| Setup | Shift・Alt・Ctrl の 7 通りの組み合わせの `TerminalModifiers`（Meta は false）、`app_cursor_keys = false` |
| Act | 6 つのカーソルキーのそれぞれを、7 通りの修飾キーで `encode` する |
| Expect | 修飾値はカーソルキーにも付く **[CM]** ／ 修飾値は終端文字の前に引数として付く **[C2]** ／ m は Shift 2、Alt 3、Shift+Alt 4、Control 5、Shift+Control 6、Alt+Control 7、Shift+Alt+Control 8 **[C3]** ／ 形は `ESC [ 1 ; m X` で、X は TC-01 と同じ終端文字 **[MD]** |

xterm の説明書が修飾キー付きのカーソルキーについて述べているのは、修飾値 2〜8 がカーソルキーにも使われること（CM）と、修飾値が終端文字の前に引数として付くこと（C2）までで、1 番目の引数 `1` を置いた具体的な形は書いていない。そこは doc コメント（MD）から取った。7 通りすべてを 6 キーで回すのは、修飾値を足し算で作る実装と組み合わせの表で作る実装のどちらも同じ結果になることを確かめるため。防ぎたいのは、`1 ;` を落として `ESC [ 5 D` を出す実装と、修飾キーを無視して `ESC [ D` を出す今の実装。VT510 の DECFNK はカーソルキーに Alt しか使わず、形も `CSI Ps1 ; Ps2 ~` なので（V4、V5、C7）、このケースは作者の決定（xterm 側）に依っている。

```rust
/// Asserts that the cursor keys held with Shift, Alt, Control, or any
/// combination of them send `CSI 1 ; m` with the key's final character
/// when DECCKM is reset, where `m` is the xterm modifier code from 2 to 8.
///
/// Case: the user presses Ctrl-Left to move back one word in a shell, or
/// Shift-End to select to the end of the line.
#[test]
fn cursor_keys_with_modifiers_send_csi_1_with_the_modifier_code_in_normal_mode() {
    for (key, final_byte) in final_bytes() {
        for (mods, code) in modifier_codes() {
            let expected = format!("\x1b[1;{code}{final_byte}");
            assert_eq!(
                key.encode(&mods, false),
                expected.as_bytes(),
                "{key:?} with {mods:?}"
            );
        }
    }
}
```

## TC-A1 — DECCKM がセットでも、修飾キー付きのカーソルキーは CSI の形のまま送る

| | |
| - | - |
| Setup | TC-03 と同じ 7 通りの `TerminalModifiers`、`app_cursor_keys = true` |
| Act | 6 つのカーソルキーのそれぞれを、7 通りの修飾キーで `encode` する |
| Expect | SS3 には変わらず、`ESC [ 1 ; m X` を送る。m と X は TC-03 と同じ **[MD]** |

`docs/references/` のどの説明書も、修飾キー付きのカーソルキーがアプリケーションモードで何を送るかを述べていない。作者が xterm(1) を足さずに doc コメントを出典にすると決めたので、doc コメント（MD）だけを出典にした `TC-A` のケースとして置いた。防ぎたいのは、DECCKM がセットのとき SS3 のまま引数を付けて `ESC O 1 ; 5 D` のような列を出す実装と、修飾キーを捨てて `ESC O D` を出す実装。

```rust
/// Asserts that the cursor keys held with a modifier send the same
/// `CSI 1 ; m` sequences when DECCKM is set, rather than switching to
/// `SS3`.
///
/// Case: the user presses Ctrl-Right or Shift-Up in a full-screen editor
/// that has turned on application cursor keys.
#[test]
fn cursor_keys_with_modifiers_keep_the_csi_form_in_application_mode() {
    for (key, final_byte) in final_bytes() {
        for (mods, code) in modifier_codes() {
            let expected = format!("\x1b[1;{code}{final_byte}");
            assert_eq!(
                key.encode(&mods, true),
                expected.as_bytes(),
                "{key:?} with {mods:?}"
            );
        }
    }
}
```

## TC-04 — Meta は修飾値に数えない

| | |
| - | - |
| Setup | `TerminalModifiers { meta: true, .. }` を、ほかの修飾キーなし・Shift と一緒・Shift+Alt+Ctrl と一緒の 3 通りで用意する |
| Act | `CursorKey::Up.encode(&meta, false)`、`CursorKey::Up.encode(&meta, true)`、`CursorKey::Up.encode(&meta_shift, false)`、`CursorKey::End.encode(&meta_all, true)` |
| Expect | Meta だけの Up は、リセットで `ESC [ A`、セットで `ESC O A` **[C5][MM][A2]** ／ Meta+Shift の Up は `ESC [ 1 ; 2 A` **[C5][MM][MD]** ／ Meta+Shift+Alt+Ctrl の End は、DECCKM がセットでも `ESC [ 1 ; 8 F` **[C5][MM][MD]** |

C5 は「alwaysUseMods を設定したときだけ Meta も数え、9〜16 になる」と述べているので、設定しない側だけを書いた（設定する側は付録「API 改定」の B1）。Meta だけのとき修飾キーなしの形になるのは C5 の条件の裏から読んだもので、doc コメント（MM）も同じことを述べている。Meta だけの Up をセット側でも確かめるのは、「何かの修飾フラグが立っているか」を `meta` も含めて判定し、SS3 から CSI に切り替えてしまう実装を防ぐため。

```rust
/// Asserts that Meta is left out of the cursor key modifier code, so Meta
/// alone keeps the unmodified sequence in either DECCKM mode and Meta with
/// other modifiers sends the code those modifiers give.
///
/// Case: a caller hands the encoder a cursor key with the Meta flag set,
/// as a press with the Super key held would carry.
#[test]
fn meta_is_left_out_of_the_cursor_key_modifier_code() {
    let meta = TerminalModifiers {
        meta: true,
        ..Default::default()
    };
    assert_eq!(CursorKey::Up.encode(&meta, false), b"\x1b[A");
    assert_eq!(CursorKey::Up.encode(&meta, true), b"\x1bOA");
    let meta_shift = TerminalModifiers {
        meta: true,
        shift: true,
        ..Default::default()
    };
    assert_eq!(CursorKey::Up.encode(&meta_shift, false), b"\x1b[1;2A");
    let meta_all = TerminalModifiers {
        meta: true,
        shift: true,
        alt: true,
        ctrl: true,
    };
    assert_eq!(CursorKey::End.encode(&meta_all, true), b"\x1b[1;8F");
}
```

テスト用のヘルパ（`mod tests` の中に置く）:

```rust
use super::*;

fn held(shift: bool, alt: bool, ctrl: bool) -> TerminalModifiers {
    TerminalModifiers {
        shift,
        alt,
        ctrl,
        meta: false,
    }
}

fn final_bytes() -> [(CursorKey, char); 6] {
    [
        (CursorKey::Up, 'A'),
        (CursorKey::Down, 'B'),
        (CursorKey::Right, 'C'),
        (CursorKey::Left, 'D'),
        (CursorKey::Home, 'H'),
        (CursorKey::End, 'F'),
    ]
}

fn modifier_codes() -> [(TerminalModifiers, u8); 7] {
    [
        (held(true, false, false), 2),
        (held(false, true, false), 3),
        (held(true, true, false), 4),
        (held(false, false, true), 5),
        (held(true, false, true), 6),
        (held(false, true, true), 7),
        (held(true, true, true), 8),
    ]
}
```

## 付録

### 契約表

`pdftotext -layout`（Git for Windows に入っている xpdf の 4.00）で抽出したテキストの行番号。ページは PDF のページ番号を計算したもの。表の行が 10 行を超えるエントリは、`verify` の幅の上限に合わせて範囲を 2 つに分け、両方を検証した。

| ID | Governs | Shape | Statement | Citation |
| --- | --- | --- | --- | --- |
| A1 | PC-Style Function Keys（カーソルキー） | mode-dependent | "The cursor keys transmit the following escape sequences depending on the mode specified via the DECCKM escape sequence." | xterm-ctlseqs.pdf p.46, L2645-2646 |
| A2 | PC-Style Function Keys（カーソルキー） | mode-dependent | Key / Normal / Application: Cursor Up CSI A SS3 A, Cursor Down CSI B SS3 B ／ Cursor Right CSI C SS3 C, Cursor Left CSI D SS3 D | xterm-ctlseqs.pdf p.46, L2648-2654 ／ L2656-2658 |
| A3 | PC-Style Function Keys（Home・End） | unconditional | "The home- and end-keys (unlike PageUp and other keys also on the 6-key editing keypad) are considered "cursor keys" by xterm. Their mode is also controlled by the DECCKM escape sequence:" | xterm-ctlseqs.pdf p.46, L2662-2664 |
| A4 | PC-Style Function Keys（Home・End） | mode-dependent | Key / Normal / Application: Home CSI H SS3 H, End CSI F SS3 F | xterm-ctlseqs.pdf p.46, L2666-2672 |
| D1 | 表 3-2 Codes Generated by Cursor Control Keys | mode-dependent | ANSI Mode の Cursor Key Mode Reset（Normal）と Set（Application）、VT52 Mode の Normal と Application: CSI A SS3 A ESC A ESC A, CSI B SS3 B ESC B ESC B ／ CSI C SS3 C ESC C ESC C, CSI D SS3 D ESC D ESC D。"* ANSI mode applies to VT200 and VT100 modes. VT52 mode is an ANSI-incompatible mode." | vt220.pdf p.72, L4566-4575 ／ L4577-4583 |
| V3 | DECCKM | mode-dependent | "If the DECCKM function is set, then the arrow keys send application sequences to the host. If the DECCKM function is reset, then the arrow keys send ANSI cursor sequences to the host." | vt510.pdf p.141, L6593-6594 |
| CM | PC-Style Function Keys | unconditional | "In the DECFNK scheme, codes 2-8 identify modifiers for function-keys and cursor-, editing-keypad keys. Unlike xterm, the VT510 limits the modifiers which can be used with cursor- and editing-keypad keys." | xterm-ctlseqs.pdf p.48, L2838-2841 |
| C2 | PC-Style Function Keys | conditional | "In normal mode, i.e., a Sun/PC keyboard when the sunKeyboard resource is false (and none of the other keyboard resources such as oldXtermFKeys resource is set), xterm encodes function key modifiers as parameters appended before the final character of the control sequence." | xterm-ctlseqs.pdf p.48, L2789-2792 |
| C3 | PC-Style Function Keys | multi-function | 2 Shift, 3 Alt, 4 Shift + Alt, 5 Control, 6 Shift + Control ／ 7 Alt + Control, 8 Shift + Alt + Control | xterm-ctlseqs.pdf p.48, L2800-2808 ／ L2810-2812 |
| C5 | PC-Style Function Keys | mode-dependent | "If the alwaysUseMods resource is set, the Meta modifier also is recognized, making parameters 9 through 16." | xterm-ctlseqs.pdf p.48, L2834-2835 |
| C13 | PC-Style Function Keys | multi-function | 9 Meta, 10 Meta + Shift, 11 Meta + Alt, 12 Meta + Alt + Shift, 13 Meta + Ctrl ／ 14 Meta + Ctrl + Shift, 15 Meta + Ctrl + Alt, 16 Meta + Ctrl + Alt + Shift（B1 の提案でだけ使う） | xterm-ctlseqs.pdf p.48, L2814-2822 ／ L2824-2828 |
| V4 | DECFNK | unconditional | "The DECFNK sequences encode the top row function keys, the six editing keypad keys, and the four cursor keys. … The four cursor keys ( , , , ) can be encoded with the Alt key." | vt510.pdf p.164, L7446-7452 |
| V5 | DECFNK | unconditional | "The cursor keys are not available unmodified since they send standard cursor key sequences, not DECFNK sequences." | vt510.pdf p.166, L7560-7561 |
| C7 | DECFNK | unconditional | Format: CSI Ps1 ; Ps2 ~ | vt510.pdf p.164, L7456 |
| X8 | VT52-Style Function Keys | unconditional | "A VT52 does not have function keys, but it does have a numeric keypad and cursor keys. They differ from the other emulations by the prefix. Also, the cursor keys do not change:"（B2 の提案でだけ使う） | xterm-ctlseqs.pdf p.50, L2986-2988 |
| X9 | VT52-Style Function Keys | unconditional | Key / Normal/Application: Cursor Up ESC A, Cursor Down ESC B ／ Cursor Right ESC C, Cursor Left ESC D（B2 の提案でだけ使う） | xterm-ctlseqs.pdf p.50, L2990-2996 ／ L2998-3000 |
| K1 | SS3 | unconditional | "ESC O Single Shift Select of G3 Character Set (SS3 is 0x8f), VT220." | xterm-ctlseqs.pdf p.5, L253-254 |
| K2 | CSI | unconditional | "ESC [ Control Sequence Introducer (CSI is 0x9b)." | xterm-ctlseqs.pdf p.6, L271-272 |
| MD | `CursorKey::encode` の doc コメント | mode-dependent | kind 2。上の台帳を参照 | cursor.rs:22-24 |
| MM | `CursorKey::encode` の doc コメント | unconditional | kind 2。上の台帳を参照 | cursor.rs:24 |

citations verified: 20/20（説明書 18 件は範囲 23 個すべて `verify`、doc コメント 2 件は `grep -F`）。X9 は最初 L2990-3000 で記録して `SPAN TOO WIDE`（11 行）になったので、引用文を変えずに範囲を L2990-2996 と L2998-3000 に分け、両方とも検証を通した。

手順の中での順位づけ:

- **修飾キーなしの矢印**：VT510 の DECCKM（V3）は「application sequences」「ANSI cursor sequences」とだけ述べ、具体的な列を書いていない。VT510 で `SS3 A`、`ESC O A`、`CSI A` を検索しても、カーソルキーの表は見つからなかった（L14156 の `CSI A` は SCO コンソールの表）。そのため、順位が次の VT220 の表 3-2（D1）を採った。xterm の表（A2）も同じ列を定めている。
- **Home・End**：VT220 と VT510 には PC キーボードの Home・End の列がない。xterm だけが定めている（A3、A4）。
- **修飾キー付きのカーソルキー**：順位の外にある xterm-ctlseqs（CM、C2、C3）と VT510（V4、V5、C7）が食い違う。作者の決定で xterm 側を採った（「仕様の矛盾」）。

### 探索した語

| 語 | レベル | 結果 |
| --- | --- | --- |
| DECCKM / app_cursor_keys | 1（メソッドの doc） | vt510 の DECCKM（V3）、vt220 の 4.6.8 と表 3-2（D1）、xterm-ctlseqs の PC-Style 節（A1、A2、A4） |
| SS3 / CSI | 1 | xterm-ctlseqs p.5-6（K1、K2） |
| Shift / Alt / Ctrl / Meta | 1 | xterm-ctlseqs p.48（C2、C3、C5、CM） |
| PC-Style Function Keys | 1（`# References`） | xterm-ctlseqs p.46-48 |
| cursor key / arrow | 2・3（enum の doc「A cursor key: one of the four arrows, Home, or End.」と `//!`「The cursor keys: the four arrows, Home, and End.」） | 上と同じ節、vt510 の DECFNK（V4、V5） |
| Home / End | 2・3 | xterm-ctlseqs p.46（A3、A4）。vt220・vt510 では見つからなかった |
| SS3 A / ESC O A / CSI A | VT510 が黙っているかの確認 | vt510 にカーソルキーの表はない（L14156 は SCO コンソール） |
| 1 ; 2 A / ; 3 A / CSI 1 ; | 修飾キー付きの形の確認 | どの説明書にもなかった（vt510 L7737 の `CSI 1 ; 1 H` は CUP の例） |
| modifyCursorKeys | 修飾キー付きの形の確認 | xterm-ctlseqs L1475・L1492・L1507・L1546（XTMODKEYS などの引数の一覧で、振る舞いは書いていない） |
| VT52 | D1 の列見出しから | xterm-ctlseqs p.50（X8、X9）、vt220 表 3-2 の VT52 Mode 列 |
| cursor key（ECMA-48） | ECMA-48 の確認 | ECMA-48 にはキーボードが送る列の定義がない（0 件） |

level 1〜3 の語で振る舞いの記述に届いたので、level 4（型名 `TerminalModifiers` の展開）には降りていない。

定義でない行として棄却したもの（確認できたもの）:

- xterm-ctlseqs：L51（目次）、L683（`CSI Ps A` は端末側が受け取る CUU で、キーが送る列ではない）、L2608-2612（Xutil.h のマクロの説明）
- vt510：L224（目次）、L1748、L2058（機能の一覧と相互参照）、L4340、L4735、L9116、L9534、L11055（モードの一覧表）、L14156（SCO コンソールの表）
- vt220：L93（目次）、L5051、L5129、L5211（モードの一覧表）

### 仕様にないもの

- 修飾キー付きのカーソルキーの形 `CSI 1 ; m X`。説明書が述べているのは、修飾値 2〜8 がカーソルキーにも使われること（CM）と、修飾値が終端文字の前に引数として付くこと（C2）までだった。1 番目の引数 `1` と終端文字は、doc コメント（MD）から取った（TC-03、TC-04）。
- DECCKM がセットのときに修飾キー付きのカーソルキーが何を送るか。どの説明書にもなく、doc コメント（MD）だけを出典にした（TC-A1）。作者は xterm(1) を `docs/references/` に足さないと決めた。

### 仕様の矛盾

**修飾キー付きのカーソルキー**（作者が 2026-10-22 に xterm 側を採ると決めた）

| 説明書 | 使える修飾キー | 形 | 出典 |
| --- | --- | --- | --- |
| xterm-ctlseqs | Shift・Alt・Ctrl のすべて（修飾値 2〜8） | 終端文字の前に修飾値の引数が付く | CM、C2、C3 |
| vt510（DECFNK） | Alt だけ | DECFNK の形 `CSI Ps1 ; Ps2 ~`。修飾キーなしでは DECFNK を使わない | V4、V5、C7 |

xterm-ctlseqs は順位の外にある説明書なので、VT510 との食い違いはこのスキルでは解消しない。xterm-ctlseqs 自身も「VT510 は xterm と違い、カーソルキーと編集キーに使える修飾キーを制限している」と書いている（CM）。作者は要件の R5 で xterm 側を選び、2026-10-22 にこの会話で、その前提でケースを出すよう求めた。TC-03・TC-A1・TC-04 はこの決定のもとで出したもので、VT510 の DECFNK の形（Alt+矢印を `CSI Ps1 ; 3 ~` で送る形）のケースは出していない。

### API 改定

下の 2 件は 1 組の変更として扱う。どちらも P（パラメータがない）のブロッカー。今のシグネチャのままにすると、C5 の「設定する側」と、D1・X8・X9 の VT52 モードの側からはケースが出ない。

**B1 — alwaysUseMods を設定した状態を作れない（C5、C13）**

- 書けない行：Setup「alwaysUseMods を設定した状態」。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, app_cursor_keys: bool, always_use_mods: bool) -> Vec<u8>`。呼び出し側の `encode_key` にも、この値をどこかから渡す必要がある（今の `VtModes` にも設定にも、該当するものはない）。`FunctionKey::encode` の B1（`docs/todo/tdd-function-encode.md`）と同じ値を使うことになる。
- 影響するケース：
  - 今のシグネチャの TC-04：書ける（設定しない側）。
  - 提案のもとでの TC-04：Setup に `always_use_mods = false` を加えるだけで、Expect は同じ。
  - 提案のもとでの新しいケース（番号なし）：Setup `always_use_mods = true`、`meta: true` ／ Act `CursorKey::Up.encode(&meta, false, true)` ／ Expect `ESC [ 1 ; 9 A` **[C5][C13]**。この形を採るなら、doc コメント（MM）も書き換えることになる。
- 補足：要件ドキュメントは、Cmd・Super 付きのキーを今どおり捨てる（Meta を数えない）と決めている。

**B2 — VT52 モードを作れない（D1、X8、X9）**

- 書けない行：Setup「端末が VT52 モードで動いている」。orzma には、VT52 モードにあたる型が見当たらなかった（`FunctionKey::encode` の B2 と同じ検索）。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, app_cursor_keys: bool, level: OperatingLevel) -> Vec<u8>`。`FunctionKey::encode` の B2 と同じ新しい型 `OperatingLevel` と、それを VT の状態から `encode_key` へ渡す経路が要る。
- 影響するケース：
  - 今のシグネチャ：VT52 モードの側は書けない。TC-01・TC-02 は ANSI モードの側として、そのまま成り立つ。
  - 提案のもとでの新しいケース（番号なし）：Setup VT52 モード、DECCKM はリセットとセットの両方 ／ Act `CursorKey::Up.encode(&none, app_cursor_keys, level)` など ／ Expect Up → `ESC A`、Down → `ESC B`、Right → `ESC C`、Left → `ESC D` で、DECCKM に左右されない **[D1][X8][X9]**。

### 出典なしの改善提案

- `CSI 1 ; m X`（終端が英字の形）は、`FunctionKey::encode` の F1〜F4 と同じ形になる。組み立てを両方から呼べる場所に置くと重複しない。仕様が求めることではなく、ケースにも影響しない。
- `keyboard.rs` の `mod tests` にある `arrows_use_csi_in_normal_cursor_mode`・`arrows_use_ss3_in_application_cursor_mode`・`home_and_end_use_csi_in_normal_cursor_mode`・`home_and_end_use_ss3_in_application_cursor_mode` は、`encode_key` を通して TC-01・TC-02 と同じ列を確かめている。どちらかに寄せると、同じ約束を 2 か所で守らずに済む。

### docs/todo との衝突

`docs/todo/tdd-function-encode.md` の「出典なしの改善提案」は、修飾値 m の計算を R5・R6 と共有するよう提案している。今回の一覧はそれと食い違わない。ほかに `CursorKey`・カーソルキー・DECCKM を扱う作業ノートはなかった。
