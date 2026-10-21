# Test cases: FunctionKey::encode

状態: 承認済み

対象は `FunctionKey::encode`（`crates/orzma_tty/src/input/keyboard/function.rs:24`、`pub fn encode(self, mods: &TerminalModifiers) -> Vec<u8>`）。参照した仕様は `xterm-ctlseqs.pdf`（PC-Style Function Keys）、`vt510.pdf`（DECFNK）、`vt220.pdf`（表 3-4）、`ECMA-48.pdf`（FNK）。citation は 21/21 件を `verify` で検証した。ケースは 3 件で、うち要確認が 2 件。API 改定の提案は 1 組（P ブロッカー 3 件）。F1〜F4 は xterm と VT510 が食い違うため、ケースを出していない（付録「仕様の矛盾」）。

この一覧は仕様から導いて自分で検証したもので、作者が 2026-10-22 に読み、会話で承認した。

ケースは High → Medium の順に並べている。High だけ読めば、仕様がはっきり述べている振る舞いはすべて含まれる。

## テストケース一覧

| # | 名前 | Source | 優先度 | 確認 |
| - | - | - | - | - |
| TC-01 | `f5_through_f12_without_a_modifier_send_their_tilde_sequences` | C1 | High | 自明 |
| TC-02 | `f5_through_f12_with_modifiers_append_the_modifier_code_as_a_parameter` | C1, C2, C3, C4 | High | 要確認 |
| TC-03 | `meta_is_left_out_of_the_function_key_modifier_code` | C5, C3 | Medium | 要確認 |

Source タグ — **C1**: xterm-ctlseqs.pdf p.47 L2547-2554（F5〜F12 の送出列）／**C2**: p.48 L2572-2575（修飾キーは終端文字の前に引数として付く）／**C3**: p.48 L2581-2587（修飾値 2〜8）／**C4**: p.48 L2598（shift-F5 の例）／**C5**: p.48 L2600-2601（alwaysUseMods のときだけ Meta を数える）。全文は付録の契約表にある。

3 件とも仕様由来で、`TC-A`（リポジトリの doc だけを根拠にするケース）はない。F5〜F12 の番号と修飾値 2〜8 は、VT510 の DECFNK（C6〜C9）も同じ値を定めている。

テストコードは `crates/orzma_tty/src/input/keyboard/function.rs` に新しく `#[cfg(test)] mod tests` を作って追加する。このファイルにはまだテストモジュールがない。`keyboard.rs` の `mod tests` が試しているのは `encode_key` で、`FunctionKey` そのものではないので、そちらには置かない。今の木にあるシグネチャに対して書いているのでコンパイルは通るはずだが、このスキルはビルドしていない。本体が `todo!()` なので、実装するまではテストは panic して失敗する。

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

テスト用のヘルパ（`mod tests` の中に置く）:

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

## TC-03 — Meta は修飾値に数えない

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

## 付録

### 契約表

`pdftotext -layout` で抽出したテキストの行番号。ページは PDF のページ番号を計算したもの。

| ID | Governs | Shape | Statement | Citation |
| --- | --- | --- | --- | --- |
| C1 | PC-Style Function Keys | unconditional | F5 CSI 1 5 ~, F6 CSI 1 7 ~, F7 CSI 1 8 ~, F8 CSI 1 9 ~, F9 CSI 2 0 ~, F10 CSI 2 1 ~, F11 CSI 2 3 ~, F12 CSI 2 4 ~ | xterm-ctlseqs.pdf p.47, L2547-2554 |
| C2 | PC-Style Function Keys | conditional | "In normal mode, … xterm encodes function key modifiers as parameters appended before the final character of the control sequence." | xterm-ctlseqs.pdf p.48, L2572-2575 |
| C3 | PC-Style Function Keys | multi-function | 2 Shift, 3 Alt, 4 Shift + Alt, 5 Control, 6 Shift + Control, 7 Alt + Control, 8 Shift + Alt + Control | xterm-ctlseqs.pdf p.48, L2581-2587 |
| C4 | PC-Style Function Keys | unconditional | "shift-F5 would be sent as CSI 1 5 ; 2 ~" | xterm-ctlseqs.pdf p.48, L2598 |
| C5 | PC-Style Function Keys | mode-dependent | "If the alwaysUseMods resource is set, the Meta modifier also is recognized, making parameters 9 through 16." | xterm-ctlseqs.pdf p.48, L2600-2601 |
| C6 | DECFNK | unconditional | Ps1: 15 F5, 17 F6, 18 F7, 19 F8, 20 F9, 21 F10, 23 F11, 24 F12（C1 と一致） | vt510.pdf p.165, L5597-5604 |
| C7 | DECFNK | unconditional | Format: CSI Ps1 ; Ps2 ~ | vt510.pdf p.164, L5570 |
| C8 | DECFNK | unconditional | Ps2: "0 or omitted none" | vt510.pdf p.165, L5615 |
| C9 | DECFNK | multi-function | Ps2: 2 Shift, 3 Alt, 4 Alt + Shift, 5 Control, 6 Control + Shift, 7 Alt + Control, 8 Alt + Control + Shift（C3 と一致） | vt510.pdf p.165, L5617-5624 |
| C10 | Top Row Function Keys | mode-dependent | F11 (ESC): VT200 mode CSI 2 3 ~, VT100/VT52 modes ESC ／ F12 (BS): VT200 mode CSI 2 4 ~, VT100/VT52 modes BS | vt220.pdf p.74, L3751-3754 |
| C11 | Top Row Function Keys | mode-dependent | 表の列見出し "VT200 Mode" と "VT100, VT52 Modes" | vt220.pdf p.74, L3735 |
| C12 | DECFNK | conditional | "Top row function keys F1 through F5 are only available when they are not used for local terminal functions." | vt510.pdf p.166, L5633-5634 |
| C13 | PC-Style Function Keys | multi-function | 9 Meta, 10 Meta + Shift, 11 Meta + Alt, 12 Meta + Alt + Shift, 13 Meta + Ctrl, 14 Meta + Ctrl + Shift, 15 Meta + Ctrl + Alt, 16 Meta + Ctrl + Alt + Shift（B1 の提案でだけ使う） | xterm-ctlseqs.pdf p.48, L2588-2595 |
| X1 | PC-Style Function Keys | unconditional | F1 SS3 P, F2 SS3 Q, F3 SS3 R, F4 SS3 S | xterm-ctlseqs.pdf p.47, L2543-2546 |
| X2 | PC-Style Function Keys | conditional | "the SS3 sent before F1 through F4 is altered to CSI when sending a function key modifier as a parameter" | xterm-ctlseqs.pdf p.48, L2575-2577 |
| X3 | DSR（の注記） | unconditional | "with the default keyboard configuration the shifted F3 key may send (with shift-, control-, alt-modifiers) CSI 1 ; 2 R , or CSI 1 ; 5 R , or CSI 1 ; 6 R" | xterm-ctlseqs.pdf p.27, L1411-1417 |
| X4 | PC-Style Function Keys | conditional | "Older versions of xterm implement different escape sequences for F1 through F4, with a CSI prefix. These can be activated by setting the oldXtermFKeys resource." | xterm-ctlseqs.pdf p.47, L2558-2560 |
| V1 | DECFNK | unconditional | Ps1: 11 F1, 12 F2, 13 F3, 14 F4 | vt510.pdf p.165, L5593-5596 |
| T1 | Top Row Function Keys | unconditional | "The first five keys (F1 through F5) labeled Hold Screen, Print Screen, Set-Up, Data/Talk, and Break, do not send codes; they are local function keys." | vt220.pdf p.18, L820-821 |
| E1 | FNK | unconditional | "FNK - FUNCTION KEY Notation: (Pn) Representation: CSI Pn 02/00 05/07" | ECMA-48.pdf p.58, L2628-2630 |
| E2 | FNK | unconditional | "FNK is a control function in which the parameter value identifies the function key which has been operated." | ECMA-48.pdf p.58, L2632-2633 |

citations verified: 21/21。E1 は最初 L2628-2633 で記録して `verify` が棄却した（dropped qualifier: no）。範囲の途中にある「No parameter default value.」の行の `No` を拾っていたので、引用文は変えずに範囲を L2628-2630 と L2632-2633（E2）に分けて記録し直し、両方とも再検証を通した。X4 も最初は L2557-2559 で記録して棄却された（20/25 matched）。引用の始まりより 1 行前から取っていて、終わりの行も足りなかったので、引用文は変えずに範囲を L2558-2560 に直し、再検証を通した。

手順の中での順位づけ:

- **F5**：VT220 は F5 をローカル機能でコードを送らないとしている（T1）。一方 VT510 は、ローカル機能に使われていなければ F5 を DECFNK の 15 として送る（C6、C12）。順位が上の VT510 を採った。
- **FNK と DECFNK**：ECMA-48 の FNK（`CSI Pn SP W`、E1・E2）と VT510 の DECFNK（`CSI Ps1 ; Ps2 ~`、C7）は形が違う。順位が上の VT510 を採った。

### 探索した語

| 語 | レベル | 結果 |
| --- | --- | --- |
| function key | 1（メソッドの doc） | xterm-ctlseqs 29 件、vt510 121 件、vt220 24 件、ECMA-48 4 件、xlib 4 件 |
| F1 / F5 / F11 / F12 | 2・3（enum と `//!` の F1〜F12） | xterm-ctlseqs の PC-Style 節、vt510 の DECFNK、vt220 の表 3-4 |
| DECFNK | function key の検索で出た語 | vt510 p.164-166 の定義 |
| alwaysUseMods / Meta | function key の検索で出た語 | xterm-ctlseqs p.48（C5、C13） |
| CSI 1 ; | F1〜F4 の修飾形の確認 | xterm-ctlseqs p.27 の DSR 注記（X3） |
| FNK | ECMA-48 の function key | ECMA-48 8.3.52（E1、E2） |
| F11 (ESC) / (ESC) / VT100 mode | VT220 の VT100 モード列について、VT510 が黙っているかの確認 | vt510 には "Function keys also work in VT100 mode."（L1736）だけがあり、送るコードは書かれていない。そのため VT220 の C10 が効く |
| local function | C12・T1 の確認 | vt510 L5633、vt220 L820 |

level 1 の語で振る舞いの記述に届いたので、level 4（型名 `TerminalModifiers` の展開）には降りていない。修飾キーの語（Shift、Alt、Control、Meta）は、level 1 で届いた節の中で見つかった。

定義でない行として棄却したもの（確認できたもの）:

- xterm-ctlseqs：L33（目次）、L1420（相互参照）
- vt510：L223（目次）、L602（表の目次）、L3661（制御機能の要約表）
- ECMA-48：L317（目次）、L2052（一覧表）
- xlib：L17320、L22771、L22783、L22801（KeySym の分類の説明で、送るバイト列ではない）

### 仕様にないもの

- 修飾キー付きの F1・F2・F4 を `CSI 1 ; m P` の形で送ること。xterm-ctlseqs が `1 ;` を明示しているのは、Shift などを押した F3 の例（X3）だけだった。F1・F2・F4 は X2 と X3 から類推するしかない。どちらにしても F1〜F4 は下の矛盾でケースを出していない。

### 仕様の矛盾

**F1〜F4 の送出列**（ケースを出していない）

| 説明書 | 修飾キーなし | 修飾キー付き | 出典 |
| --- | --- | --- | --- |
| xterm-ctlseqs（PC-Style） | `SS3 P`、`SS3 Q`、`SS3 R`、`SS3 S` | SS3 が CSI に変わり、引数が付く。F3 の例は `CSI 1 ; 2 R` | X1、X2、X3 |
| vt510（DECFNK） | `CSI 11 ~`、`CSI 12 ~`、`CSI 13 ~`、`CSI 14 ~` | `CSI 11 ; m ~` など | V1、C7、C8 |
| vt220（表 3-4） | ローカル機能で、コードを送らない | — | T1（VT510 より順位が下なので、VT510 に負ける） |

xterm-ctlseqs は順位の外にある説明書なので、VT510 との食い違いはこのスキルでは解消しない。xterm-ctlseqs 自身も、`CSI 11 ~` 系は oldXtermFKeys で選べる古い形だと書いている（X4）。参考までに書くと、要件ドキュメント `docs/requirements/2026-10-21-xterm-key-encoding.md` の R1・R3 は xterm 側（`SS3 P`、`CSI 1 ; m P`）を選んでいる。どちらを採るか作者が決めれば、再実行で F1〜F4 のケースを出せる。

### API 改定

下の 3 件は 1 組の変更として扱う。どれも P（パラメータがない）のブロッカー。今のシグネチャのままにすると、C5 の「設定する側」、C10 の VT100/VT52 モードの側、C12 の「ローカル機能に使われている側」からはケースが出ない。

**B1 — alwaysUseMods を設定した状態を作れない（C5、C13）**

- 書けない行：Setup「alwaysUseMods を設定した状態」。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, always_use_mods: bool) -> Vec<u8>`。呼び出し側の `encode_key` にも、この値をどこかから渡す必要がある（今の `VtModes` にも設定にも、該当するものはない）。
- 影響するケース：
  - 今のシグネチャの TC-03：書ける（設定しない側）。
  - 提案のもとでの TC-03：Setup に `always_use_mods = false` を加えるだけで、Expect は同じ。
  - 提案のもとでの新しいケース（番号なし）：Setup `always_use_mods = true`、`meta: true` ／ Act `FunctionKey::F5.encode(&meta, true)` ／ Expect `ESC [ 1 5 ; 9 ~` **[C5][C13]**、Meta+Shift の F5 は `ESC [ 1 5 ; 1 0 ~` **[C5][C13]**。
- 補足：要件ドキュメントは、Cmd・Super 付きのキーを今どおり捨てる（Meta を数えない）と決めている。

**B2 — VT100/VT52 モードを作れない（C10、C11）**

- 書けない行：Setup「端末が VT100 モードまたは VT52 モードで動いている」。orzma には、動作レベルや VT52 モードにあたる型が見当たらなかった（`orzma_vt` と `orzma_tty` で `vt52`、`DECANM`、`DECSCL`、`conformance` を検索した）。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, level: OperatingLevel) -> Vec<u8>`。新しい型 `OperatingLevel`（VT200 以上か、VT100/VT52 か）と、それを VT の状態から `encode_key` へ渡す経路が要る。
- 影響するケース：
  - 今のシグネチャ：VT100/VT52 モードの側は書けない。TC-01 は VT200 モードの側として、そのまま成り立つ。
  - 提案のもとでの新しいケース（番号なし）：Setup VT100 モード ／ Act `FunctionKey::F11.encode(&none, level)`、`FunctionKey::F12.encode(&none, level)` ／ Expect F11 は `ESC`（0x1b）**[C10]**、F12 は `BS`（0x08）**[C10]**。

**B3 — F5 がローカル機能に使われている状態を作れない（C12）**

- 書けない行：Setup「F5 がローカル機能に割り当てられている」。Expect「F5 は何も送らない」。今の戻り値 `Vec<u8>` では「送らない」を空の列でしか表せず、ローカル機能かどうかを渡す入力もない。
- 最小の変更：`pub fn encode(self, mods: &TerminalModifiers, local: bool) -> Option<Vec<u8>>`。呼び出し側の `encode_key` の戻り値の扱いも変わる。
- 影響するケース：
  - 今のシグネチャ：ローカル機能の側は書けない。TC-01・TC-02 は、ローカル機能に使われていない側として成り立つ。
  - 提案のもとでの新しいケース（番号なし）：Setup `local = true` ／ Act `FunctionKey::F5.encode(&none, true)` ／ Expect `None` **[C12]**。

### 出典なしの改善提案

- 修飾値 m（Shift・Alt・Ctrl から 2〜8 を作る値）の計算は、要件の R5・R6（修飾キー付きの矢印・Home・End・Insert・Delete・PageUp・PageDown）でも使う。`FunctionKey` の中ではなく、両方から呼べる場所に置くと重複しない。これは仕様が求めることではなく、ケースにも影響しない。
