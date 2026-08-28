# Test cases: KeypadKey::encode

対象は `crates/orzma_tty/src/input/keyboard/keypad.rs:58` の
`impl KeypadKey` にある `pub(super) fn encode(&self, mode: KeypadMode) -> Vec<u8>`。

参照した仕様は `docs/references/xterm-ctlseqs.pdf` p.49 の
"VT220-Style Function Keys" のキーパッド表。同 p.46-47 の "PC-Style Function Keys"
表は**この方法の仕様ではない** — 理由は付録「PC-Style 表の読み違い」に記録した。

**citations verified: 22/22**（xterm 19 / vt510 3）。Phase 4 でケース一覧10件を承認、
signature は現状維持。その後 revise を4件適用した。うち3件は族ごとの統合、
4件目は上記の読み違いの訂正で、旧 TC-04〜TC-07 が1件に置き換わり4件になった。

**このRustはビルドしていない。** 静的には `mod tests` にそのまま貼れる形にしてあるが、
コンパイルを通したわけではない。通れば `KeypadKey::encode` の本体が現在 `todo!()` なので
全テストが panic する — これは TDD の RED として意図した状態。貼って動くコードでは
なく転記用の提案として読むこと。

このリストは仕様由来かつ自己検証のみで、他の誰もレビューしていない。信頼する前に
引用を1つ2つ実際に引いて確かめてほしい。

## テストケース一覧

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `numeric_mode_sends_characters_rather_than_sequences` | C1, C2 | High |
| TC-02 | `application_mode_shifts_the_numeric_byte_into_the_ss3_range` | C3, C4 | High |
| TC-03 | `application_mode_equal_uses_ss3_x` | C5 | High |
| TC-04 | `application_mode_digits_use_ss3_letters` | C3, C7 | High |

Source タグ — **C1**: xterm p.49 L2648-2664（Numeric 列）／**C2**: p.49 L2643（Enter 行の
Numeric 列）／**C3**: p.49 L2648-2653（`* + , - . /` の6行）／**C4**: p.49 L2643（Enter 行の
Application 列）／**C5**: p.49 L2664／**C7**: p.49 L2654-2663（数字10行）。
全エントリの全文は付録の契約表にある。

4件すべてが仕様由来（kind 1 のみ）。リポジトリ側の doc に依存した `TC-A` 系のケースは
無く、`Damage` タグも使っていない（戻り値が `Vec<u8>` で `Option<DamageSpan>` ではない）。

優先度は全件 High。この方法は 18キー × 2モード の36通りすべてをマニュアルが明記して
いるため、「仕様が直接述べていない」ケースが存在せず、優先度軸が識別力を持たない。

テストコードは `crates/orzma_tty/src/input/keyboard/keypad.rs` の末尾に
`#[cfg(test)] mod tests` を新設して置く。このファイルにはまだテストモジュールが無い。
`use super::*;` を書けば `KeypadKey` / `KeypadMode` はそのまま使える
（`encode` は `pub(super)` だが同一ファイル内なので届く）。隣の `keyboard.rs` の
既存テストの `assert_eq!(encode_key(...), b"...".to_vec())` の様式に合わせてある。

4件のうち3件は表を回すループなので、同じテスト内で複数行が同時に壊れると最初の
1行で止まり、残りは次の実行まで見えない。挙動のカバレッジは1キー1テストに分けた
場合と変わらず、違うのは切り分けの粒度だけ。

## TC-01 — Numeric モードではシーケンスではなく文字を送る

| | |
| - | - |
| Setup | なし（純関数） |
| Act | `key.encode(KeypadMode::Numeric)` を18キーすべてについて |
| Expect | `/` → `/`／`*` → `*`／`-` → `-`／`+` → `+`／`,` → `,`／`=` → `=`／`.` → `.`／`0`–`9` → `"0"`–`"9"` **[C1]** ／ `Enter` → `\r`（0x0D） **[C2]** |

Numeric 列が「シーケンスではなく文字」であることが、この方法の出発点。ここが崩れると
残り3ケースの前提も崩れる。VT510 Table 8-3 を実装すると `/` `*` `-` がこの列でも
`SS3 Q` / `SS3 R` / `SS3 S` を送るので、そのままそれを落とす。

Enter を同じ表に入れているのは、Numeric モードの契約が「文字を送る」1つであり、
CR(0x0D) もその文字だから。CR は ASCII における Enter そのもので例外ではなく、
隣の `keyboard.rs` の `TerminalKey::Enter` も `vec![0x0d]` を返す。18行そろえて
おくと、Enter を Numeric 側で未処理にする実装もこの1本で落ちる。

```rust
/// Asserts that numeric mode sends the character each keypad key types
/// rather than an escape sequence.
///
/// Case: a shell prompt is waiting for input and the user types a figure
/// and an operator on the numeric keypad, then presses its Enter to
/// submit the line.
#[test]
fn numeric_mode_sends_characters_rather_than_sequences() {
    let cases: [(KeypadKey, &[u8]); 18] = [
        (KeypadKey::Divide, b"/"),
        (KeypadKey::Multiply, b"*"),
        (KeypadKey::Subtract, b"-"),
        (KeypadKey::Add, b"+"),
        (KeypadKey::Comma, b","),
        (KeypadKey::Equal, b"="),
        (KeypadKey::Decimal, b"."),
        (KeypadKey::Enter, b"\r"),
        (KeypadKey::Zero, b"0"),
        (KeypadKey::One, b"1"),
        (KeypadKey::Two, b"2"),
        (KeypadKey::Three, b"3"),
        (KeypadKey::Four, b"4"),
        (KeypadKey::Five, b"5"),
        (KeypadKey::Six, b"6"),
        (KeypadKey::Seven, b"7"),
        (KeypadKey::Eight, b"8"),
        (KeypadKey::Nine, b"9"),
    ];
    for (key, expected) in cases {
        assert_eq!(key.encode(KeypadMode::Numeric), expected.to_vec());
    }
}
```

## TC-02 — Application モードでは Numeric のバイトを 0x40 上げて SS3 に載せる

| | |
| - | - |
| Setup | なし |
| Act | `key.encode(KeypadMode::Application)` を `* + , - /` と `Enter` について |
| Expect | `*` → `\x1bOj`／`+` → `\x1bOk`／`,` → `\x1bOl`／`-` → `\x1bOm`／`/` → `\x1bOo` **[C3]** ／ `Enter` → `\x1bOM` **[C4]** |

Application モードの符号化は「Numeric で送るバイト + 0x40」で、`=` を除く17キー
すべてがこれに従う（`*`=0x2A→`j`、`+`=0x2B→`k`、`,`=0x2C→`l`、`-`=0x2D→`m`、
`/`=0x2F→`o`、Enter は CR=0x0D→`M`）。Enter だけ結果が大文字になるのは、起点が
印字文字ではなく C0 制御文字だからで、別の規則ではない。

数字と `.` も同じ規則に従うが、そちらは別の誤りを防ぐ役目があるので TC-04 に
分けてある。この規則に従わない `=` は TC-03。

VT510 の位置読み替えを入れると `+` が `SS3 l`、`-` が `SS3 S` になるので、
`,` と `-` の2行が同時に落ちる。`,` を含めているのはそのため — `+` と `,` が
別々のバイトを持つことが、この2つを取り違えていない証拠になる。

`SS3 M`（0x4D）と `SS3 m`（0x6D）が同じテストの中で隣り合うのも意図的で、
大文字小文字を取り違える実装はここで落ちる。

```rust
/// Asserts that the keypad operators and Enter send `SS3` followed by
/// their numeric-mode byte raised by 0x40.
///
/// Case: a full-screen spreadsheet has taken the keypad over with
/// `DECKPAM` and the user types an operator into a cell, then presses the
/// keypad Enter to commit it.
#[test]
fn application_mode_shifts_the_numeric_byte_into_the_ss3_range() {
    let cases: [(KeypadKey, &[u8]); 6] = [
        (KeypadKey::Multiply, b"\x1bOj"),
        (KeypadKey::Add, b"\x1bOk"),
        (KeypadKey::Comma, b"\x1bOl"),
        (KeypadKey::Subtract, b"\x1bOm"),
        (KeypadKey::Divide, b"\x1bOo"),
        (KeypadKey::Enter, b"\x1bOM"),
    ];
    for (key, expected) in cases {
        assert_eq!(
            key.encode(KeypadMode::Application),
            expected.to_vec()
        );
    }
}
```

## TC-03 — Application モードの `=` は SS3 X

| | |
| - | - |
| Setup | なし |
| Act | `KeypadKey::Equal.encode(KeypadMode::Application)` |
| Expect | `\x1bOX` **[C5]** |

`=` は 0x3D なので + 0x40 なら 0x7D = `}` になるが、実際は `X`。TC-02 の規則を
計算で実装すると、このキーだけが静かに間違う。

外れる理由は分かっている。xterm の実装は計算ではなく1本の文字列テーブルで、
`kypd_apl[keysym - XK_KP_Space]` を引く。X11 のキーパッド keysym は
`0xFF00 | (ASCII + 0x80)` なので添字は Numeric モードの文字の ASCII そのものになり、
テーブルは添字1–26 が `A`–`Z`、33–58 が `a`–`z`、59–61 が未定義マーカーの `XXX`。
`9`(0x39) が添字57で `y`、`:`(0x3A) が添字58で `z` — ここでアルファベットが尽きる。
`=` は 0x3D なので `z` の3つ先に落ち、未定義マーカーの `X` がそのまま出力される。
つまり規則が壊れているのではなく、規則の適用範囲を超えている。

Macintosh のテンキーには `=` が実在するので、到達不能な行ではない。

```rust
/// Asserts that the keypad `=` uses `SS3 X`, which the character-plus-0x40
/// rule the other keys follow would not produce.
///
/// Case: a Macintosh keyboard, whose keypad carries `=`, is driving an
/// application that has taken the keypad over.
#[test]
fn application_mode_equal_uses_ss3_x() {
    assert_eq!(
        KeypadKey::Equal.encode(KeypadMode::Application),
        b"\x1bOX".to_vec()
    );
}
```

## TC-04 — Application モードの数字と `.` も SS3 + 小文字

| | |
| - | - |
| Setup | なし |
| Act | `key.encode(KeypadMode::Application)` を `.` と `0`–`9` について |
| Expect | `.` → `\x1bOn` **[C3]** ／ `0`–`9` → `\x1bOp`・`\x1bOq`・`\x1bOr`・`\x1bOs`・`\x1bOt`・`\x1bOu`・`\x1bOv`・`\x1bOw`・`\x1bOx`・`\x1bOy` **[C7]** |

規則としては TC-02 と同一で、分けているのは**別の誤りを名指しで防ぐため**。

このリストは当初 `0`→`CSI 2~`（Insert）、`2`→`CSI B`（↓）、`7`→`SS3 H`（Home）、
`.`→`CSI 3~`（Del）を期待する4ケースになっていた。xterm の "PC-Style Function Keys"
表を DECKPAM の表として読んだためで、これは誤りだった（付録「PC-Style 表の読み違い」）。
同じ誤りは実装系にも存在し、WezTerm が `termwiz/src/input.rs` でこの変換を実装して
いる。しかも `Numpad9 => "\x1b[6"`（PageUp なら `\x1b[5`）という転記バグを抱えており、
あの表を仕様として書き写すと何が起きるかの実例になっている。

数字キーは数字の識別子として届く。DECKPAM は届いた識別子の符号化を選ぶモードで
あって、識別子そのものを Home や ↓ に変換するものではない。Home や PageUp は
キーマップ層が別の keysym として送り出す別のキーであり、`TerminalKey` 側の
識別子として DECCKM で分岐させるのが正しい配置になる。

```rust
/// Asserts that the keypad digits and the decimal key send `SS3` followed
/// by their numeric-mode byte raised by 0x40, rather than the editing
/// sequences their gray legends name.
///
/// Case: an application holding the keypad receives presses on the digit
/// keys and the decimal key.
#[test]
fn application_mode_digits_use_ss3_letters() {
    let cases: [(KeypadKey, &[u8]); 11] = [
        (KeypadKey::Decimal, b"\x1bOn"),
        (KeypadKey::Zero, b"\x1bOp"),
        (KeypadKey::One, b"\x1bOq"),
        (KeypadKey::Two, b"\x1bOr"),
        (KeypadKey::Three, b"\x1bOs"),
        (KeypadKey::Four, b"\x1bOt"),
        (KeypadKey::Five, b"\x1bOu"),
        (KeypadKey::Six, b"\x1bOv"),
        (KeypadKey::Seven, b"\x1bOw"),
        (KeypadKey::Eight, b"\x1bOx"),
        (KeypadKey::Nine, b"\x1bOy"),
    ];
    for (key, expected) in cases {
        assert_eq!(
            key.encode(KeypadMode::Application),
            expected.to_vec()
        );
    }
}
```

## 付録

### 契約表

出典はすべて `docs/references/xterm-ctlseqs.pdf`、`pdftotext -layout` 抽出に対する
行番号。C0–C7 は p.49 の VT220-Style 表、C11–C12 は p.46-47。

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C0 | VT220-Style keypad | mode-dependent | "The VT102/VT220 application keypad transmits unique escape sequences in application mode, which are distinct from the cursor and scrolling keypad" | p.49, L2635-2637 |
| C1 | Numeric 列 | mode-dependent | 各キーが自身の文字を送る（`* + , - . /`、`0`–`9`、`=`） | p.49, L2648-2664 |
| C2 | Enter, Numeric | mode-dependent | "Enter CR" | p.49, L2643 |
| C3 | `* + , - . /`, App | mode-dependent | "* (multiply) * SS3 j no + (add) + SS3 k no , (comma) , SS3 l yes - (minus) - SS3 m yes . (period) . SS3 n yes / (divide) / SS3 o no" | p.49, L2648-2653 |
| C4 | Enter, App | mode-dependent | "Enter CR SS3 M yes" | p.49, L2643 |
| C5 | `=`, App | mode-dependent | "= (equal) = SS3 X no" | p.49, L2664 |
| C7 | `0`–`9`, App | mode-dependent | "0 0 SS3 p yes 1 1 SS3 q yes 2 2 SS3 r yes 3 3 SS3 s yes 4 4 SS3 t yes 5 5 SS3 u yes 6 6 SS3 v yes 7 7 SS3 w yes 8 8 SS3 x yes 9 9 SS3 y yes" | p.49, L2654-2663 |
| C11 | NumLock | 対象外 | "Use the NumLock key to override the application mode" | p.46, L2505 |
| C12 | PF1–PF4, Space, Tab | blocker P（現状維持） | "Not all keys are present on the Sun/PC keypad (e.g., PF1, Tab), but are supported by the program"／"PF1 SS3 P SS3 P yes PF2 SS3 Q SS3 Q yes PF3 SS3 R SS3 R yes PF4 SS3 S SS3 S yes" | p.46, L2507-2508 ／ p.49, L2644-2647 |

C3 は TC-02（`* + , - /`）と TC-04（`.`）の両方が出典として引く。エントリを分けて
いないのは、マニュアルの表がこの6行を1つのまとまりとして並べているためで、引用の
単位を表に合わせてある。

C6 / C8 / C9 / C10 は欠番。PC-Style 表から導出していた旧エントリで、読み違いの訂正
時に削除した。

### 探索した語

| Level | 出所 | 語 | 到達先 |
| - | - | - | - |
| 1 | `KeypadKey::encode` の `///` | "PC-Style Function Keys"（doc がURLで名指し） | xterm p.46 — 到達したが、後述の理由で仕様としては採用せず |
| 2 | 引数型 `KeypadKey` の doc | "PC-layout numeric keypad" | xterm p.46 — 同上 |
| 2 | 引数型 `KeypadKey` の doc | "VT220-Style Function Keys" | xterm p.49 — 到達。**これが本方法の仕様** |
| 2 | 引数型 `KeypadMode`（orzma_vt） | DECKPAM / DECKPNM / DECNKM | xterm p.46 L2504 で言及 — C0 相当。vt510 p.178/180/191 の定義自体は本方法の戻り値を規定しないため契約表には入れず |
| 3 | ファイル `//!` | "byte sequence the PTY expects" | 一般語につき到達せず |
| 4 | 方法名 + 型名 | "application keypad" | xterm p.46 L2503、p.49 L2635 — 到達 |
| 4 | 方法名 + 型名 | "numeric keypad" | xterm p.46 L2503、vt510 p.369 — 到達 |

Level 1 と Level 2 の一方が誤った表へ導いた点は、この探索の教訓として残す。
`KeypadKey::encode` の doc comment が PC-Style 節の URL を指していたのが探索の起点
だったので、VT220-Style へ差し替え済み（`keyboard/keypad.rs`）。

### 仕様にないもの

- **C11（NumLock override）** — 引用は検証済み（p.46 L2505）だが、この方法の
  signature は解決済みの `mode` を受け取るため、NumLock がどちらのモードを届けるかは
  呼び出し側の責務であり、`KeypadKey::encode` では設定も観測もできない。O ブロッカーでは
  なく層が違う。`KeypadKey` から `NumLock` を外した判断もこれに沿う。
  なお macOS のテンキーには NumLock が存在せず、XKB も Mac 用に
  `type "KEYPAD" { modifiers = None; map[None] = Level2; }`（数字を無条件に選ぶ）を
  持つので、主対象プラットフォームでは常に無効化されたのと同じ状態になる。
- 引用が取れずに落としたエントリは無い。検証に失敗した引用も無い（22/22）。

### PC-Style 表の読み違い

このリストは当初 p.46-47 の "PC-Style Function Keys" 表を DECKPAM の仕様として
読み、旧 TC-04〜TC-07 を導出していた。誤りだったので、何を読み違えたかを記録する。

**xterm には DECKPAM で分岐するキーパッド経路が1本しかない**（`input.c:1390`）:

```c
} else if (IsKeypadKey(kd.keysym)) {
    if (keypad_mode) {
        reply.a_type = ANSI_SS3;
        reply.a_final = (Char) (kypd_apl[kd.keysym - XK_KP_Space]);
```

`kypd_apl` は `XK_KP_0`–`XK_KP_9` を `SS3 p`–`SS3 y` に写す。PC-Style 表に載って
いる `CSI 2~` / `CSI B` / `SS3 F` はここからは絶対に出ない。それらは NumLock が
OFF のときに来る別の keysym を書き換える経路の出力である（`input.c:1177`）:

```c
if (kd.keysym >= XK_KP_Home && kd.keysym <= XK_KP_Begin) {
    kd.keysym += (KeySym) (XK_Home - XK_KP_Home);
}
```

XKB は物理キー1つに2つの keysym を持たせ、NumLock で段を選ぶ:

```
key <KP7> { [ KP_Home, KP_7 ] };
type "KEYPAD" { modifiers = NumLock; map[None] = Level1; map[NumLock] = Level2; };
```

つまり PC-Style 表は、**物理キー1つが持つ2つの役割を1行に併記した刻印の説明**で
あって、モードの表ではない。表の Application 列が `SS3 H`/`SS3 F`（DECCKM
application 形）と `CSI A`/`CSI B`/`CSI E`（DECCKM normal 形）を混在させていて、
5キーすべてが同じ経路・同じフラグを通る以上**単一のモード状態では produce できない**
ことが、その証拠になっている。

同じ読み違いは実装系にも存在する。WezTerm（`termwiz/src/input.rs`）はこの表を
数字→編集キー変換として実装し、`Numpad9 => "\x1b[6"`（PageUp なら `\x1b[5`）という
転記バグまで抱えている。

実装系の分布（一次ソース確認済み）:

| 実装 | Application モードの数字キー |
| - | - |
| xterm, foot, ghostty, urxvt | `SS3 p`–`SS3 y` |
| Terminal.app | `SS3 p`–`SS3 y`。ただし表は厳密に VT100 で、xterm が足した `* + / =` と PF1–PF4 を持たない |
| iTerm2 | `SS3 p`–`SS3 y`。ただし `TERM` に "xterm" を含むときのみ DECKPAM を受理する |
| Windows Terminal | `SS3 p`–`SS3 y` を実装しているが、機能フラグ `Feature_KeypadModeEnabled` が `features.xml` で `AlwaysDisabled`（`Dev` ビルドのみ有効）なので、出荷版は数字のまま |
| VTE, Konsole, kitty, Alacritty | DECKPAM の数字符号化を実装していない |
| WezTerm | PC-Style 編集シーケンス（唯一の例）。ただし macOS バックエンドはイベントの文字列から `KeyCode::Char('1')` を組むため、この表は macOS では到達しない |

Terminal.app については出荷バイナリ
（`/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal`）を走査して確認した。
カーソルキー表の直後に NUL 区切りの連続テーブルが並んでいる:

```
\EOH \EOF \EOA \EOB \EOD \EOC | \E[A \E[B \E[D \E[C | \EOl \EOm \EOn \EOp \EOq \EOr \EOs \EOt \EOu \EOv \EOw \EOx \EOy
```

`\EOj` `\EOk` `\EOo` `\EOX`（`* + / =`）と `\EOP`–`\EOS`（PF1–PF4）は0件。これは
xterm との不一致ではなく、Terminal.app が VT100 を emulate していて、それらのキーが
VT100 のキーパッドに存在しないためである。orzma は xterm 互換を狙うので TC-02 と
TC-03 はそのままでよい。

terminfo も一致する。`xterm` / `xterm-256color` は
`ka1=\EOw kb2=\EOu kc1=\EOq kpZRO=\EOp kpDOT=\EOn` に解決し、ncurses は `kb2` を
`\EOE`(Begin) から `\EOu`(数字5) へ意図的に移して `\EOE` を別能力 `kbeg`/`kp5` として
残している。xterm と ncurses の両方を維持している Thomas Dickey による裁定と読める。
macOS 同梱の `nsterm` / `nsterm-256color` も
`ka1=\EOq ka3=\EOs kb2=\EOr kc1=\EOp kc3=\EOn kent=\EOM smkx=\E[?1h\E=` と、SS3 形で
記述している（3x3 の割り当ては VT100 と1行ずれているので、`ka1` を VT100 互換の
隅キーとして読まないこと）。`vim` も `src/term.c` に
`{K_K0, "\033O*p"}` … `{K_K9, "\033O*y"}` をハードコードしている。

**主対象プラットフォームでは反例が存在しない。** macOS で DECKPAM を実装している
2つ（Terminal.app と iTerm2）はどちらも `SS3 p`–`SS3 y` を送り、唯一の反例である
WezTerm はその経路に macOS から到達できない。

### 仕様の矛盾

`docs/references/` の2つのマニュアルが、同じキーについて別のバイトを規定している。
これは上の読み違いとは別の、実在する食い違い。

| | xterm p.49 (VT220-Style) | vt510.pdf p.369 (Table 8-3) |
| - | - | - |
| `/` App | `SS3 o` | `SS3 Q`（PF2） |
| `*` App | `SS3 j` | `SS3 R`（PF3） |
| `-` App | `SS3 m` | `SS3 S`（PF4） |
| `+` App | `SS3 k` | `SS3 l`（DEC comma） |
| `/` `*` `-` Numeric | `/` `*` `-` | `SS3 Q` `SS3 R` `SS3 S` |

VT510 側の引用（すべて検証済み）:

- p.369, L10518-10521 — "Num Lock PF1 SS3 P SS3 P / PF2 SS3 Q SS3 Q * PF3 SS3 R SS3 R - PF4 SS3 S SS3 S"
- p.369, L10523 — `+ , "+" SS3 l`
- p.369, L10537-10538 — "In PC Style, when the numeric keypad is in Application Mode, the numeric keypad keys send the same sequences as the corresponding keys on the VT layout as shown in Table 8-3"

3つめが効いていて、VT510 は PC 配列のキーボードを挿しても Application モードでは
Table 8-3 を使えと明記している。DEC のキーパッドと PC のテンキーは同じ位置に違う
キーが並ぶので、上段4キーを PF1–PF4 の位置の関数として振る舞わせる設計になっている。

**このスキルのマニュアル優先順位（vt510 > vt220 > ECMA-48）に従えば VT510 が勝つ。**
本ドキュメントが xterm から導出しているのは、その優先順位を作者判断で上書きした
結果であり、根拠は互換性である。上の実装分布の通り、この位置読み替えを採用している
実装は存在しない。

矛盾自体はここで解決していない。記録として残す。

### API 改定

**C12 — left as is。** PF1–PF4 / Space / Tab は xterm の表に載っており引用も検証
済みだが、`KeypadKey` に対応するバリアントが無いため
`Act: KeypadKey::Pf1.encode(KeypadMode::Application)` が書けない（kind P）。
最小の変更は `KeypadKey` に `Pf1, Pf2, Pf3, Pf4, Space, Tab` を足すことで、
3ケース（PF1–PF4 はモード非依存につき1件に畳まれる、Space 1件、Tab 1件）が
書けるようになる。

作者は現状維持を選択。これらのキーは PC/Mac のテンキーに物理的に存在せず、
`bevy_input::KeyCode` にも対応する識別子が無いため、ホスト層から到達できない。

### 出典なしの改善提案

- **統合を3回適用した。** Phase 4 の承認後、作者の提起で (1) Numeric モードの Enter を
  TC-01 へ、(2) Application モードの Enter を TC-02 へ、(3) `.` を編集シーケンス群へ、
  順に畳んだ。根拠は共通で、両者が setup も action も同一でスキルの merge 規則に
  該当すること、および既存テストに複数キーを1つの契約の下にまとめる前例が多いこと
  （`vt220_style_keys_use_tilde_sequences` L249 が3キー、
  `arrows_use_csi_in_normal_cursor_mode` L307 が4キー、
  `ctrl_letter_collapses_to_c0_byte` L347 が3マッピング）。
- **PC-Style 表の読み違いを訂正した（4件目の revise）。** 旧 TC-04〜TC-07 の4件が
  TC-04 の1件に置き換わった。数字と `.` を TC-02 に完全統合しなかったのは、規則が
  同一でも、この行が実装系に実在する誤り（WezTerm）を名指しで防ぐ役目を持つため。
  規則としての統合より、誤りの記録としての分離を優先した。
- 残る4ケースをこれ以上畳む余地は乏しい。TC-02 と TC-04 は同じ規則なので1件にできる
  が、上の理由で分けている。TC-03 は規則の例外、TC-01 は別のモード。

### docs/todo との衝突

衝突なし。`docs/todo/esc-dispatch.md` が2箇所で keypad に触れているが、いずれも
DECKPAM/DECKPNM のディスパッチ側の話で、エンコード表とは重ならない。

- `esc-dispatch.md:30` — `xterm-256color` の terminfo が `smkx=\E[?1h\E=` /
  `rmkx=\E[?1l\E>` である件。`KeypadKey::encode` がどちらのモードで呼ばれるかを決める
  上流の話。なお同じ terminfo エントリが `kc1=\EOq` 等を持つことは、上の
  「PC-Style 表の読み違い」の裏付けとして働く。
- `esc-dispatch.md:38` — 典拠として vt510.pdf p.178 / p.180 を挙げている。本
  ドキュメントが p.369 で VT510 から離れる判断をしていることとは、対象が違うので
  矛盾しない。
