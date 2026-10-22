# Test cases: EditingKey::encode

状態: 承認済み

対象は `EditingKey::encode`（`crates/orzma_tty/src/input/keyboard/editing.rs:26`、`pub fn encode(self, mods: &TerminalModifiers) -> Vec<u8>`）。参照した仕様は `xterm-ctlseqs.pdf`（PC-Style Function Keys、VT220-Style Function Keys、DECSET 1037）、`vt220.pdf`（表 3-1）、`vt510.pdf`（DECFNK）。citation は 19/19 件（説明書 17 件、リポジトリの doc コメント 2 件）を検証した。ケースは 3 件で、すべて要確認。API 改定の提案は 1 組（P ブロッカー 3 件）。

作者は 2026-10-22 にこの会話で、修飾キー付きの編集キーで xterm と VT510 が食い違う点（VT510 の DECFNK は編集キーに Ctrl と Alt しか使わず、Shift を区別しない）は、要件 `docs/requirements/2026-10-21-xterm-key-encoding.md` の R6 どおり xterm 側を採ると決めた。xterm(1) は `docs/references/` に足さない。

この一覧は仕様から導いて自分で検証したもので、作者が 2026-10-22 に読み、会話で承認した。

ケースは High → Medium の順に並べている。High だけ読めば、仕様がはっきり述べている振る舞いはすべて含まれる。

## テストケース一覧

| # | 名前 | Source | 優先度 | 確認 |
| - | - | - | - | - |
| TC-01 | `editing_keys_without_a_modifier_send_their_tilde_sequences` | E2, T1, M2 | High | 要確認 |
| TC-02 | `editing_keys_with_modifiers_append_the_modifier_code_as_a_parameter` | C2, CM, C3, ED | High | 要確認 |
| TC-03 | `meta_is_left_out_of_the_editing_key_modifier_code` | C5, EM, ED | Medium | 要確認 |

Source タグ — **E2**: xterm-ctlseqs.pdf p.50（編集キーの表）／**T1**: vt220.pdf p.71 表 3-1（編集キーが送る列）／**M2**: xterm-ctlseqs.pdf p.22（DECRST 1037 で Delete は VT220 の Remove を送る）／**C2**: p.48（修飾値は終端文字の前に引数として付く）／**CM**: p.48（修飾値 2〜8 は編集キーにも使う）／**C3**: p.48（修飾値 2〜8）／**C5**: p.48（alwaysUseMods のときだけ Meta を数える）／**ED**・**EM**: `EditingKey::encode` の doc コメント（下の台帳）。全文は付録の契約表にある。

3 件とも仕様由来で、`TC-A` はない。TC-02・TC-03 は、作者が xterm 側を採ると決めたことで書けるようになったケースでもある。TC-01 は 2 つの説明書の表と DECSET 1037 のリセット側を合わせて使うので、自明の条件（契約エントリが 1 つ）に当たらず要確認にした。編集キーが DECCKM に左右されないこと（E1、E3）は、`encode` が DECCKM を受け取らないことでシグネチャがすでに守っているので、ケースにしていない。Rust では、CSI を 7 ビットの `ESC [` で書いている（K2）。

テストコードは `crates/orzma_tty/src/input/keyboard/editing.rs` に新しく `#[cfg(test)] mod tests` を作って追加する。このファイルにはまだテストモジュールがない。`keyboard.rs` の `mod tests` にある Insert などのテストが試しているのは `encode_key` で、`EditingKey` そのものではないので、そちらには置かない。ヘルパ `held`・`numbers`・`modifier_codes` は、この `mod tests` の中に新しく置く。今の木にあるシグネチャに対して書いているのでコンパイルは通るはずだが、このスキルはビルドしていない。本体が `todo!()` なので、実装するまではテストは panic して失敗する。

### kind 2 の台帳

```
ED — crates/orzma_tty/src/input/keyboard/editing.rs:18-20, EditingKey::encode
     "With any of them held, the keys send `CSI n ; m ~` with the same `n`, where `m` is 1 plus Shift 1, Alt 2, and Ctrl 4."
     justifies: 修飾キー付きの形が `CSI n ; m ~` で、n が修飾キーなしのときと同じであること
EM — crates/orzma_tty/src/input/keyboard/editing.rs:20, EditingKey::encode
     "Meta does not count toward `m`."
     justifies: Meta だけでは修飾キーなしの形のままで、ほかの修飾キーと一緒なら Meta を除いた m になること
```

## TC-01 — 修飾キーなしの編集キーは、それぞれの番号の `CSI n ~` を送る

| | |
| - | - |
| Setup | `TerminalModifiers::default()` |
| Act | `EditingKey::Insert.encode(&none)` … `EditingKey::PageDown.encode(&none)` |
| Expect | Insert → `ESC [ 2 ~`、Delete → `ESC [ 3 ~`、PageUp → `ESC [ 5 ~`、PageDown → `ESC [ 6 ~` **[E2][T1]** ／ Delete は DEL（0x7f）ではなく VT220 の Remove の列を送る **[M2]** |

xterm の表（E2）は PC キーボードの名前（Insert・Delete・PageUp・PageDown）で、VT220 の表 3-1（T1）は VT220 の名前（Insert Here・Remove・Prev Screen・Next Screen）で同じ列を定めている。VT510 は DECFNK の Ps1 の表を持っているが、今回の抽出では番号とキー名が別々の行に分かれて照合できず（`docs/todo/tdd-function-encode.md` の付録と同じ事情）、順位が次の VT220 の表を採った。番号が連続していない（4 がない）ところが間違えやすい点で、Insert から 1 ずつ数える実装は PageUp で 4 を出す。Delete については、xterm の DECSET 1037 がセットなら DEL を送る（M1）。orzma にはこのモードがないので、リセット側（M2）の振る舞いとして書いた（セット側は付録「API 改定」の B3）。

```rust
/// Asserts that the editing keys without a modifier send `CSI 2 ~`,
/// `CSI 3 ~`, `CSI 5 ~`, and `CSI 6 ~`.
///
/// Case: the user presses Delete to remove the character under the cursor,
/// or PageDown to scroll a pager, with no modifier key held.
#[test]
fn editing_keys_without_a_modifier_send_their_tilde_sequences() {
    let none = TerminalModifiers::default();
    let cases: [(EditingKey, &[u8]); 4] = [
        (EditingKey::Insert, b"\x1b[2~"),
        (EditingKey::Delete, b"\x1b[3~"),
        (EditingKey::PageUp, b"\x1b[5~"),
        (EditingKey::PageDown, b"\x1b[6~"),
    ];
    for (key, expected) in cases {
        assert_eq!(key.encode(&none), expected, "{key:?}");
    }
}
```

## TC-02 — 修飾キー付きの編集キーは、修飾値を 2 番目の引数として `CSI n ; m ~` を送る

| | |
| - | - |
| Setup | Shift・Alt・Ctrl の 7 通りの組み合わせの `TerminalModifiers`（Meta は false） |
| Act | 4 つの編集キーのそれぞれを、7 通りの修飾キーで `encode` する |
| Expect | 修飾値は編集キーにも付く **[CM]** ／ 修飾値は終端の `~` の前に `; m` として付き、n は TC-01 と同じ **[C2][ED]** ／ m は Shift 2、Alt 3、Shift+Alt 4、Control 5、Shift+Control 6、Alt+Control 7、Shift+Alt+Control 8 **[C3]** |

C2 は条件付きの記述（修飾キーがあれば引数が付く）で、付かない側は TC-01 が受け持つ。7 通りすべてを 4 キーで回すのは、Shift を含む組み合わせ（2・4・6・8）を落とさないため。VT510 は編集キーの Shift を区別せず（V6）、Ctrl と Alt だけを符号化する（V4）ので、Shift+PageUp を `ESC [ 5 ~` のまま送る実装は VT510 には合うが、作者の決定（xterm 側）には合わない。防ぎたいのはその実装と、修飾キーを無視する今の実装。

```rust
/// Asserts that the editing keys held with Shift, Alt, Control, or any
/// combination of them send `CSI n ; m ~` with the key's own number, where
/// `m` is the xterm modifier code from 2 to 8.
///
/// Case: the user presses Ctrl-PageDown to switch to the next tab in an
/// editor, or Ctrl-Delete to delete the next word.
#[test]
fn editing_keys_with_modifiers_append_the_modifier_code_as_a_parameter() {
    for (key, number) in numbers() {
        for (mods, code) in modifier_codes() {
            let expected = format!("\x1b[{number};{code}~");
            assert_eq!(
                key.encode(&mods),
                expected.as_bytes(),
                "{key:?} with {mods:?}"
            );
        }
    }
}
```

## TC-03 — Meta は修飾値に数えない

| | |
| - | - |
| Setup | `TerminalModifiers { meta: true, .. }` を、ほかの修飾キーなし・Shift と一緒・Shift+Alt+Ctrl と一緒の 3 通りで用意する |
| Act | `EditingKey::Insert.encode(&meta)`、`EditingKey::PageUp.encode(&meta_shift)`、`EditingKey::PageDown.encode(&meta_all)` |
| Expect | Meta だけの Insert は `ESC [ 2 ~` で、引数を付けない **[C5][EM]** ／ Meta+Shift の PageUp は `ESC [ 5 ; 2 ~` **[C5][EM][ED]** ／ Meta+Shift+Alt+Ctrl の PageDown は `ESC [ 6 ; 8 ~` **[C5][EM][ED]** |

C5 は「alwaysUseMods を設定したときだけ Meta も数え、9〜16 になる」と述べているので、設定しない側だけを書いた（設定する側は付録「API 改定」の B1）。Meta だけのとき修飾キーなしの形になるのは C5 の条件の裏から読んだもので、doc コメント（EM）も同じことを述べている。防ぎたいのは、`meta` を 8 として足してしまう実装（Meta+Shift の PageUp で `; 10` を出す）。

```rust
/// Asserts that Meta is left out of the editing key modifier code, so
/// Meta alone adds no parameter and Meta with other modifiers sends the
/// code those modifiers give.
///
/// Case: a caller hands the encoder an editing key with the Meta flag set,
/// as a press with the Super key held would carry.
#[test]
fn meta_is_left_out_of_the_editing_key_modifier_code() {
    let meta = TerminalModifiers {
        meta: true,
        ..Default::default()
    };
    assert_eq!(EditingKey::Insert.encode(&meta), b"\x1b[2~");
    let meta_shift = TerminalModifiers {
        meta: true,
        shift: true,
        ..Default::default()
    };
    assert_eq!(EditingKey::PageUp.encode(&meta_shift), b"\x1b[5;2~");
    let meta_all = TerminalModifiers {
        meta: true,
        shift: true,
        alt: true,
        ctrl: true,
    };
    assert_eq!(EditingKey::PageDown.encode(&meta_all), b"\x1b[6;8~");
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

fn numbers() -> [(EditingKey, u8); 4] {
    [
        (EditingKey::Insert, 2),
        (EditingKey::Delete, 3),
        (EditingKey::PageUp, 5),
        (EditingKey::PageDown, 6),
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

`pdftotext -layout`（Git for Windows に入っている xpdf の 4.00）で抽出したテキストの行番号。ページは PDF のページ番号を計算したもの。表の行が 10 行を超えるエントリは、`verify` の幅の上限に合わせて範囲を分け、すべてを検証した。

| ID | Governs | Shape | Statement | Citation |
| --- | --- | --- | --- | --- |
| E1 | VT220-Style Function Keys（編集キー） | unconditional | "The VT220 provides a 6-key editing keypad, which is analogous to that on the PC keyboard. It is not affected by DECCKM or DECKPNM/DECKPAM:" | xterm-ctlseqs.pdf p.49, L2939-2940 |
| E2 | VT220-Style Function Keys（編集キー） | unconditional | Key / Normal / Application: Insert CSI 2 ~ CSI 2 ~, Delete CSI 3 ~ CSI 3 ~ ／ PageUp CSI 5 ~ CSI 5 ~, PageDown CSI 6 ~ CSI 6 ~ | xterm-ctlseqs.pdf p.50, L2941-2947 ／ L2953-2955 |
| E3 | PC-Style Function Keys | unconditional | "The home- and end-keys (unlike PageUp and other keys also on the 6-key editing keypad) are considered "cursor keys" by xterm." | xterm-ctlseqs.pdf p.46, L2662-2663 |
| T1 | 表 3-1 Codes Generated by Editing Keys | mode-dependent | 列見出し "Key / VT200 Mode / VT100, VT52 Modes" ／ INSERT HERE CSI 2 ~, REMOVE CSI 3 ~ ／ PREV SCREEN CSI 5 ~, NEXT SCREEN CSI 6 ~（VT100・VT52 モードの列は "--"） | vt220.pdf p.71, L4537-4541 ／ L4546-4550 ／ L4555-4559 |
| T2 | 表 3-1 Codes Generated by Editing Keys | unconditional | "In VT100 or VT52 modes the editing keys do not generate codes."（B2 の提案でだけ使う） | vt220.pdf p.71, L4561 |
| M1 | DECSET 1037 | mode-dependent | "Ps = 1 0 3 7 Send DEL from the editing-keypad Delete key, xterm."（B3 の提案でだけ使う） | xterm-ctlseqs.pdf p.20, L1115-1116 |
| M2 | DECRST 1037 | mode-dependent | "Ps = 1 0 3 7 Send VT220 Remove from the editing-keypad Delete key, xterm." | xterm-ctlseqs.pdf p.22, L1258-1259 |
| C2 | PC-Style Function Keys | conditional | "In normal mode, i.e., a Sun/PC keyboard when the sunKeyboard resource is false (and none of the other keyboard resources such as oldXtermFKeys resource is set), xterm encodes function key modifiers as parameters appended before the final character of the control sequence." | xterm-ctlseqs.pdf p.48, L2789-2792 |
| CM | PC-Style Function Keys | unconditional | "In the DECFNK scheme, codes 2-8 identify modifiers for function-keys and cursor-, editing-keypad keys. Unlike xterm, the VT510 limits the modifiers which can be used with cursor- and editing-keypad keys." | xterm-ctlseqs.pdf p.48, L2838-2841 |
| C3 | PC-Style Function Keys | multi-function | 2 Shift, 3 Alt, 4 Shift + Alt, 5 Control, 6 Shift + Control ／ 7 Alt + Control, 8 Shift + Alt + Control | xterm-ctlseqs.pdf p.48, L2800-2808 ／ L2810-2812 |
| C4 | PC-Style Function Keys | unconditional | "For example, shift-F5 would be sent as CSI 1 5 ; 2 ~"（同じ `~` の形の例。ケースのタグには使っていない） | xterm-ctlseqs.pdf p.48, L2832 |
| C5 | PC-Style Function Keys | mode-dependent | "If the alwaysUseMods resource is set, the Meta modifier also is recognized, making parameters 9 through 16." | xterm-ctlseqs.pdf p.48, L2834-2835 |
| C13 | PC-Style Function Keys | multi-function | 9 Meta, 10 Meta + Shift, 11 Meta + Alt, 12 Meta + Alt + Shift, 13 Meta + Ctrl ／ 14 Meta + Ctrl + Shift, 15 Meta + Ctrl + Alt, 16 Meta + Ctrl + Alt + Shift（B1 の提案でだけ使う） | xterm-ctlseqs.pdf p.48, L2814-2822 ／ L2824-2828 |
| V4 | DECFNK | unconditional | "The DECFNK sequences encode the top row function keys, the six editing keypad keys, and the four cursor keys. … The six editing keypad keys ( Find , Insert Here , Remove , Select , Prev , Next ), can be encoded unmodified or in combinations with the Ctrl and Alt keys." | vt510.pdf p.164, L7446-7452 |
| V6 | DECFNK | unconditional | "Shift is not distinguished for the editing and numeric keypad keys by default so it does not interfere with touch typing patterns. This means that shifted keys send the same DECFNK sequence as the unshifted keys." | vt510.pdf p.166, L7556-7558 |
| C7 | DECFNK | unconditional | Format: CSI Ps1 ; Ps2 ~ | vt510.pdf p.164, L7456 |
| K2 | CSI | unconditional | "ESC [ Control Sequence Introducer (CSI is 0x9b)." | xterm-ctlseqs.pdf p.6, L271-272 |
| ED | `EditingKey::encode` の doc コメント | conditional | kind 2。上の台帳を参照 | editing.rs:18-20 |
| EM | `EditingKey::encode` の doc コメント | unconditional | kind 2。上の台帳を参照 | editing.rs:20 |

citations verified: 19/19（説明書 17 件は範囲 22 個すべて `verify`、doc コメント 2 件は `grep -F`）。T1 は最初、列見出しから REMOVE までを L4537-4547 で記録して `SPAN TOO WIDE`（11 行）になったので、引用文を変えずに列見出し（L4537-4541）・Insert Here と Remove（L4546-4550）・Prev Screen と Next Screen（L4555-4559）の 3 つに分け、すべて検証を通した。T1 の表は、`-layout` の抽出で番号の列が 1 行ずれて並ぶ（INSERT HERE の行の `CSI 2 ~` が REMOVE の行に出る）ので、引用文はその並びのまま照合し、契約表には組み直した対応を書いた。

手順の中での順位づけ:

- **修飾キーなしの編集キー**：VT510 の DECFNK（V4、C7）は、編集キーを修飾キーなしでも DECFNK の形で送ると述べているが、Ps1 の番号の表は今回の抽出で照合できなかった。そのため番号は、順位が次の VT220 の表 3-1（T1）から取った。xterm の表（E2）も同じ番号を定めている。
- **修飾キー付きの編集キー**：順位の外にある xterm-ctlseqs（CM、C2、C3）と VT510（V4、V6、C7）が食い違う。作者の決定で xterm 側を採った（「仕様の矛盾」）。

### 探索した語

| 語 | レベル | 結果 |
| --- | --- | --- |
| DECCKM | 1（メソッドの doc） | xterm-ctlseqs p.46（E3）、p.49（E1） |
| CSI / `~` | 1 | xterm-ctlseqs p.6（K2）、p.48（C2） |
| Shift / Alt / Ctrl / Meta | 1 | xterm-ctlseqs p.48（C2、C3、C5、CM） |
| PC-Style Function Keys | 1（`# References`） | xterm-ctlseqs p.46-48 |
| editing keypad / editing keys | 2・3（enum の doc「A key of the editing keypad: Insert, Delete, PageUp, or PageDown.」と `//!`） | xterm-ctlseqs p.49-50（E1、E2）、p.20・22（M1、M2）、vt220 表 3-1（T1、T2）、vt510 の DECFNK（V4、V6） |
| Insert / Delete / PageUp / PageDown | 2・3 | xterm-ctlseqs p.50（E2）。PC キーボードのテンキーの表（L2708-2730）にも同じ列があるが、テンキーのキーなので使っていない |
| Insert Here / Remove / Prev Screen / Next Screen | VT220・VT510 の呼び名 | vt220 表 3-1（T1）、vt510 の DECFNK（V4） |
| 1037 | E2 の Delete の確認で出た語 | xterm-ctlseqs p.20・22（M1、M2）。orzma_vt と orzma_tty には該当するモードがない |
| editing key（ECMA-48） | ECMA-48 の確認 | キーボードが送る列の定義はない（0 件） |

level 1〜3 の語で振る舞いの記述に届いたので、level 4（型名 `TerminalModifiers` の展開）には降りていない。

定義でない行として棄却したもの（確認できたもの）:

- xterm-ctlseqs：L2611（Xutil.h のマクロの説明）、L2708・L2712・L2718・L2730（テンキーのアプリケーションモードの表で、編集キーではない）
- vt510：L442、L604、L658、L663、L14009-14012（目次と図の目次・図の見出し）、L2743-2745（文字セットの切り替えの説明）、L7343（別の機能の説明）、L13447（SCO コンソールの表）、L13565-13580（6.1.1.3 は SCO コンソールモードの節の中にあるキーの対応表なので、VT モードの根拠には使わなかった）
- vt220：L60、L164（目次）、L716、L777、L816、L4939（キーボードの構成の説明）

### 仕様にないもの

- なし。今回のケースの `Expect:` の行には、どれも説明書の出典がある。ED は形を補強するために添えた。

### 仕様の矛盾

**修飾キー付きの編集キー**（作者が 2026-10-22 に xterm 側を採ると決めた）

| 説明書 | 使える修飾キー | Shift | 出典 |
| --- | --- | --- | --- |
| xterm-ctlseqs | Shift・Alt・Ctrl のすべて（修飾値 2〜8） | 区別する | CM、C2、C3 |
| vt510（DECFNK） | Ctrl と Alt だけ | 区別しない（Shift 付きも修飾キーなしと同じ列を送る） | V4、V6、C7 |

xterm-ctlseqs は順位の外にある説明書なので、VT510 との食い違いはこのスキルでは解消しない。xterm-ctlseqs 自身も「VT510 は xterm と違い、カーソルキーと編集キーに使える修飾キーを制限している」と書いている（CM）。作者は要件の R6 で xterm 側を選び、2026-10-22 にこの会話で、その前提でケースを出すよう求めた。TC-02・TC-03 はこの決定のもとで出したもので、VT510 の振る舞い（Shift を無視する）のケースは出していない。

### API 改定

下の 3 件は 1 組の変更として扱う。どれも P（パラメータがない）のブロッカー。今のシグネチャのままにすると、C5 の「設定する側」、T1・T2 の VT100・VT52 モードの側、M1 の DECSET 1037 がセットの側からはケースが出ない。

**B1 — alwaysUseMods を設定した状態を作れない（C5、C13）**

- 書けない行：Setup「alwaysUseMods を設定した状態」。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, always_use_mods: bool) -> Vec<u8>`。`FunctionKey::encode` と `CursorKey::encode` の B1 と同じ値を、`encode_key` から渡すことになる。
- 影響するケース：
  - 今のシグネチャの TC-03：書ける（設定しない側）。
  - 提案のもとでの TC-03：Setup に `always_use_mods = false` を加えるだけで、Expect は同じ。
  - 提案のもとでの新しいケース（番号なし）：Setup `always_use_mods = true`、`meta: true` ／ Act `EditingKey::Insert.encode(&meta, true)` ／ Expect `ESC [ 2 ; 9 ~` **[C5][C13][C2]**。この形を採るなら、doc コメント（EM）も書き換えることになる。
- 補足：要件ドキュメントは、Cmd・Super 付きのキーを今どおり捨てる（Meta を数えない）と決めている。

**B2 — VT100・VT52 モードを作れない（T1、T2）**

- 書けない行：Setup「端末が VT100 モードまたは VT52 モードで動いている」。Expect「編集キーは何も送らない」。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, level: OperatingLevel) -> Vec<u8>`。`FunctionKey::encode` と `CursorKey::encode` の B2 と同じ新しい型 `OperatingLevel` と、それを VT の状態から `encode_key` へ渡す経路が要る。「何も送らない」は空の `Vec` で表せる。
- 影響するケース：
  - 今のシグネチャ：VT100・VT52 モードの側は書けない。TC-01・TC-02 は VT200 モードの側として、そのまま成り立つ。
  - 提案のもとでの新しいケース（番号なし）：Setup VT100 モード ／ Act `EditingKey::Insert.encode(&none, level)` … `EditingKey::PageDown.encode(&none, level)` ／ Expect 4 キーとも空の列 **[T2]**。

**B3 — DECSET 1037 がセットの状態を作れない（M1、M2）**

- 書けない行：Setup「DECSET 1037（Delete で DEL を送る）がセットされている」。orzma_vt と orzma_tty には、このモードにあたるものが見当たらなかった（`1037` などで検索した）。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, delete_sends_del: bool) -> Vec<u8>`。`VtModes` にモード 1037 を加え、`encode_key` から渡す経路が要る。
- 影響するケース：
  - 今のシグネチャの TC-01：書ける（リセット側の M2 として）。
  - 提案のもとでの TC-01：Setup に `delete_sends_del = false` を加えるだけで、Expect は同じ。
  - 提案のもとでの新しいケース（番号なし）：Setup `delete_sends_del = true` ／ Act `EditingKey::Delete.encode(&none, true)` ／ Expect DEL（0x7f）**[M1]**。修飾キー付きの Delete が何を送るかは M1 に書かれていないので、このケースは修飾キーなしに限る。

### 出典なしの改善提案

- `CSI n ; m ~`（`~` で終わる形）は、`FunctionKey::encode` の F5〜F12 と同じ形になる。組み立てを両方から呼べる場所に置くと重複しない。仕様が求めることではなく、ケースにも影響しない。
- `keyboard.rs` の `mod tests` にある `vt220_style_keys_use_tilde_sequences` と `editing_keypad_ignores_cursor_mode` は、`encode_key` を通して TC-01 と同じ列を確かめている。後者は `encode_key` が DECCKM を編集キーに渡さないことを確かめていて、`EditingKey::encode` 側にはこの約束を確かめる場所がない（シグネチャが守っている）ので、`encode_key` 側に残す意味がある。

### docs/todo との衝突

`docs/todo/tdd-function-encode.md` の「出典なしの改善提案」は、修飾値 m の計算を R5・R6 と共有するよう提案している。今回の一覧はそれと食い違わない。ほかに `EditingKey`・編集キー・1037 を扱う作業ノートはなかった。
