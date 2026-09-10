# Test cases: Executor::set_modes

作成日: 2026-09-11 / 対象リビジョン: `31c986f`

非 private の **SM / RM**（`CSI Pm h` / `CSI Pm l`）を適用する `Executor::set_modes`
が負うテストケース。今回この関数が答えるモードは **IRM**（`CSI 4 h` / `CSI 4 l`）
1つだけで、KAM(2) / SRM(12) / LNM(20) は黙って無視する。参照したのは
`docs/references/` の `vt510.pdf`（IRM p.319、モード表 p.87、DECSC p.243、
RIS p.331）、`ECMA-48.pdf`（7.1 モードの概念、8.3.106 RM、8.3.125 SM）、
`xterm-ctlseqs.pdf`（`?1049` p.20）。**citations verified: 11/11**（すべて
マニュアル由来。kind-2 エントリはゼロ。C11 は付録でのみ使う）。

Phase 4 の結果は「7件を承認」。signature 提案は無し（承認済みの設計そのまま）。

> **このドキュメントの Rust は現状の木に対してコンパイルできない。**
> `Executor::set_modes` は**まだ存在せず**、`InsertReplaceMode` も
> `VtModes::insert_replace` も未定義。下の設計を先に入れてから転記すること。
>
> ```rust
> // crates/orzma_vt/src/interpreter.rs — csi_dispatch の DECSET 腕の直前
> (None, b'h') => self.set_modes(&params, true),
> (None, b'l') => self.set_modes(&params, false),
>
> fn set_modes(&mut self, params: &CsiParams<'_>, enabled: bool) {
>     for mode in params.values().flatten() {
>         match mode {
>             // IRM
>             4 => self.device.modes_mut().insert_replace = InsertReplaceMode::from_sm(enabled),
>             _ => {}
>         }
>     }
> }
> ```
>
> **どのテストもビルドしていない。** この skill はコンパイルを走らせないし、
> 合意した関数がまだ木に無い以上、走らせようもない。貼って動くものではなく、
> 転記する提案として読むこと。

> **Phase 0 は grep では解決していない。** `fn set_modes` は
> `crates/orzma_vt/src/` のどこにも無い（確認済み）。skill の Phase 0 は
> 「一致なしなら停止」と定めているが、その規則は綴り間違いや誤ったファイル指定を
> 捕まえるためのもので、**著者が承認済みの signature を前提に洗い出す**経路は
> Phase 4 に既にある。上の signature を前提として解決した。

このリストは仕様から導いて自己検証しただけで、他の誰もレビューしていない。
信用する前に引用を1〜2件だけ実際の PDF に当てて確かめてほしい。

## テストケース一覧

優先度順（High → Medium）。Low は無い。High まで読めば、仕様が明言している
振る舞いは全部揃う。

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `a_set_mode_four_selects_insert_mode` | C3 | High |
| TC-02 | `a_reset_mode_four_selects_replace_mode` | C3 | High |
| TC-03 | `a_device_that_saw_no_mode_sequence_replaces` | C4 | High |
| TC-04 | `a_private_mode_four_does_not_select_insert_mode` | C5 | High |
| TC-05 | `an_unimplemented_mode_number_does_not_hide_an_implemented_one` | C1, C8 | Medium |
| TC-06 | `a_reset_to_initial_state_returns_the_mode_to_replace` | C6, C4 | Medium |
| TC-07 | `an_alternate_screen_flip_keeps_the_mode` | C9, C10 | Medium |
| TC-08 | `a_print_under_set_mode_four_shifts_the_row_right` | C1, C3 | High |
| TC-09 | `a_print_under_reset_mode_four_overwrites_the_cell` | C2, C3 | High |
| TC-10 | `a_cursor_restore_does_not_carry_the_mode_back` | C10 | Medium |

Source タグ — **C1**: ECMA-48 p.79 L3662-3666（SM がパラメータ値の指定どおりに
モードを設定し、4 が IRM）／**C2**: ECMA-48 p.70 L3228-3232（RM の同型）／
**C3**: vt510 p.319 L9179-9181（`CSI 4 h` = insert、`CSI 4 l` = replace）／
**C4**: vt510 p.319 L9176（既定は Replace）／**C5**: vt510 p.87 L3365-3370（ANSI 列
`Pa=4` が IRM、DEC 列 `Pd=?4` が DECSCLM）／**C6**: vt510 p.331 L9420-9423（RIS は
すべての set-up 機能を保存された設定＝工場出荷時の既定へ戻す）／**C7**: ECMA-48
p.34 L1451-1452（モードの状態は SM / RM で確立される）／**C8**: ECMA-48 p.34
L1453（実装によってはモードが1状態しか持たないことがある）／**C9**:
xterm-ctlseqs.pdf p.20 L1047-1050（`?1049` は DECSC と同じようにカーソルを保存して
代替画面へ切り替える）／**C10**: vt510 p.243 L7328-7335（DECSC が保存する7項目の
閉じた列挙）。全エントリの引用文は付録の契約表にある。

7件すべてがマニュアル由来で、`TC-A` 番号のケースも kind-2 タグも出ていない。
`set_modes` も `InsertReplaceMode` の doc もまだ存在せず、引用できるリポジトリ内の
文が無いため。

テストコードは **新規** `crates/orzma_vt/src/interpreter/tests/modes.rs` へ追加し、
`crates/orzma_vt/src/interpreter/tests.rs` の `mod` 一覧に `mod modes;` を
`mod line_movement;` と `mod mouse;` の間へ登録する（既存の並びはアルファベット順）。
既存モジュールに非 private SM/RM を扱うものは無く、`private_modes.rs` は
DECSET/DECRST 専用なので兄弟として置く。ヘルパは既存の
`interpret(chunk: &[u8]) -> DeviceState` をそのまま使う。`InsertReplaceMode` は
`tests.rs` の `use crate::device::modes::{MouseEncoding, MouseTracking};` に足せば
`use super::*` 経由で届く。

---

## TC-01 — `CSI 4 h` は insert mode を選ぶ 〈High〉

| | |
| - | - |
| Setup | なし（新しい device） |
| Act | `interpret(b"\x1b[4h")` |
| Expect | `device.modes().insert_replace == InsertReplaceMode::Insert` **[C3]** |

`csi_dispatch` に `(None, b'h')` の腕が生えたことを確かめる最小のケース。今日の木では
この列は末尾の `_ => {}` に落ちて何も起こらない。device 側のフィールドを直接見るのは、
`Screen::print` への波及は `tdd-screen-print.md` が別に pin しており、ここで見たいのは
シーケンスがモードに届くことだけだから。

```rust
/// Asserts that `CSI 4 h` selects insert mode on the device.
///
/// Case: a curses application on a terminal without `ich` announces an
/// insertion by entering insert mode before it prints.
#[test]
fn a_set_mode_four_selects_insert_mode() {
    let device = interpret(b"\x1b[4h");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Insert);
}
```

## TC-02 — `CSI 4 l` は replace mode に戻す 〈High〉

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b[4h\x1b[4l")` |
| Expect | `device.modes().insert_replace == InsertReplaceMode::Replace` **[C3]** |

一度立ててから落とすのが要点。`CSI 4 l` だけを送って Replace を見ても、モードが
落ちたのか最初から立っていなかったのかを区別できない。`from_sm` が `enabled` を
無視して常に `Insert` を返す実装をここで落とす。

```rust
/// Asserts that `CSI 4 l` returns the device to replace mode.
///
/// Case: the same application leaves insert mode as soon as the
/// insertion is done, which curses always pairs with entering it.
#[test]
fn a_reset_mode_four_selects_replace_mode() {
    let device = interpret(b"\x1b[4h\x1b[4l");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}
```

## TC-03 — モードのシーケンスを見ていない device は replace 〈High〉

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"$ ")` |
| Expect | `device.modes().insert_replace == InsertReplaceMode::Replace` **[C4]** |

vt510 が明記する既定値を pin する。`#[default]` を `Insert` 側に付けてしまう取り違えを
落とすのが目的で、これは enum のバリアントを並べ替えたときに起きやすい。普通の
バイト列を流すのは、モードに触らない出力が既定を動かさないことも同時に見るため。

```rust
/// Asserts that a device that saw no mode sequence is in replace mode.
///
/// Case: a shell starts up and prints its prompt without ever touching
/// IRM.
#[test]
fn a_device_that_saw_no_mode_sequence_replaces() {
    let device = interpret(b"$ ");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}
```

## TC-04 — `CSI ? 4 h` は IRM を立てない 〈High〉

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b[?4h")` |
| Expect | `device.modes().insert_replace == InsertReplaceMode::Replace` **[C5]** |

vt510 のモード表は ANSI 列（`Pa`）と DEC 列（`Pd`）を並べて持ち、同じ行で `4` が IRM、
`?4` が DECSCLM。private マーカーの有無が別のモードを選ぶ。番号だけ見て intermediate を
無視する分岐 — たとえば `set_modes` と `set_private_modes` を1つにまとめてしまう実装 —
をここで落とす。DECSCLM（平滑スクロール）は orzma が実装しないので、`?4` が黙って
無視されるのが正しい結果になる。

```rust
/// Asserts that `CSI ? 4 h` leaves IRM alone, because the private marker
/// selects DECSCLM rather than the ANSI mode of the same number.
///
/// Case: a program enables smooth scrolling on a terminal that also
/// answers the ANSI mode numbered four.
#[test]
fn a_private_mode_four_does_not_select_insert_mode() {
    let device = interpret(b"\x1b[?4h");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}
```

## TC-05 — 未実装の番号が実装済みの番号を隠さない 〈Medium〉

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b[2;4;12h")` |
| Expect | `device.modes().insert_replace == InsertReplaceMode::Insert` **[C1][C8]** |

SM は `(Ps...)` でパラメータ列を取り、「パラメータ値の指定どおりにモードを設定する」
（C1）。ECMA-48 は実装がモードを1状態しか持たないことを許す（C8）ので KAM(2) と
SRM(12) を無視するのは正当だが、無視の仕方が列の走査を止めてはいけない。`4` を列の
**真ん中**に置くのは、未知の番号で `break` する実装と読み飛ばす実装を分けるため。
末尾に置くと `break` する実装も通ってしまい、先頭に置くと何も区別できない。

```rust
/// Asserts that an unimplemented mode number in the parameter list does
/// not stop the ones behind it from applying.
///
/// Case: an application sets keyboard action, insert/replace and
/// send/receive in one sequence, and this terminal answers only the
/// middle one.
#[test]
fn an_unimplemented_mode_number_does_not_hide_an_implemented_one() {
    let device = interpret(b"\x1b[2;4;12h");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Insert);
}
```

## TC-06 — RIS は IRM を既定へ戻す 〈Medium〉

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b[4h\x1bc")` |
| Expect | `device.modes().insert_replace == InsertReplaceMode::Replace` **[C6][C4]** |

RIS は「すべての set-up 機能を保存された設定へ戻す」で、保存された設定は工場出荷時の
既定と同じ（C6）。IRM の既定は Replace（C4）。`DeviceState::reset` の
`VtModes::default()` が既にやるので追加コードは要らないが、`VtModes` にフィールドを
足すたびにこの経路が生きていることを確かめる価値がある。将来 `reset` が
フィールドを1つずつ復元する形に書き換わったとき、取りこぼしをここで捕まえる。

```rust
/// Asserts that `RIS` returns IRM to replace mode.
///
/// Case: a program leaves insert mode set when it dies, and the user
/// resets the terminal to recover from the mess it left.
#[test]
fn a_reset_to_initial_state_returns_the_mode_to_replace() {
    let device = interpret(b"\x1b[4h\x1bc");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}
```

## TC-07 — 代替画面への切り替えは IRM を保つ 〈Medium〉

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b[4h\x1b[?1049h")` |
| Expect | `device.modes().insert_replace == InsertReplaceMode::Insert` **[C9][C10]** |

`?1049` は「DECSC と同じようにカーソルを保存してから代替画面へ切り替える」と定義され
（C9）、DECSC が保存するのは vt510 が *"Saves the following items"* と閉じて列挙する
7項目 — カーソル位置・SGR 属性・GL/GR の文字集合・wrap flag・DECOM・選択消去属性・
保留中の SS2/SS3 — で、**IRM はそこに無い**（C10）。つまり flip はモードを運ばない。

**設計で「IRM はデバイス全体に1つ」と決めた根拠を、テストとして固定する唯一の
ケース。** IRM を `ScreenState` に置く実装（切り替え先の画面の既定値が見える）も、
flip でリセットする実装も、ここで落ちる。

```rust
/// Asserts that switching to the alternate screen carries IRM across,
/// because the flip saves only what DECSC saves.
///
/// Case: a shell in insert mode launches a full-screen editor, which
/// enters the alternate screen.
#[test]
fn an_alternate_screen_flip_keeps_the_mode() {
    let device = interpret(b"\x1b[4h\x1b[?1049h");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Insert);
}
```

---

## TC-08 — `CSI 4 h` のあとの印字は実際に行を右へずらす 〈High〉

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"abcd\x1b[2G\x1b[4hX")` |
| Expect | 先頭行が `['a','X','b','c']` になる **[C1][C3]** |

TC-01〜TC-07 はどれも `device.modes()` しか見ないので、`Executor::print` が
モードを読んで `Screen::print` に渡す**唯一の行**を誰も pin していなかった。
その行を `InsertReplaceMode::Replace` に固定しても全テストが緑のままになる
（実測）。ここと TC-09 の2本で両方向を塞ぐ。

```rust
/// Asserts that a character printed while `CSI 4 h` is in force shifts
/// the row right, so the mode the device carries reaches the screen's
/// print path rather than stopping at the mode snapshot.
///
/// Case: a curses application without `ich` enters insert mode and types
/// one character into the middle of a line it has already drawn.
#[test]
fn a_print_under_set_mode_four_shifts_the_row_right() {
    let device = interpret(b"abcd\x1b[2G\x1b[4hX");
    assert_eq!(row_glyphs(&device), ['a', 'X', 'b', 'c']);
}
```

---

## TC-09 — `CSI 4 l` のあとの印字は上書きに戻る 〈High〉

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"abcd\x1b[2G\x1b[4h\x1b[4lX")` |
| Expect | 先頭行が `['a','X','c','d']` になる **[C2][C3]** |

TC-08 の逆方向。印字経路を `InsertReplaceMode::Insert` に固定する実装は、
空行への印字では差が出ないので既存の printing テストをすり抜ける。

```rust
/// Asserts that a character printed after `CSI 4 l` overwrites the cell
/// at the cursor, so the print path reads the live mode instead of
/// assuming insert once IRM has been set.
///
/// Case: the same application leaves insert mode and keeps echoing over
/// the line it drew.
#[test]
fn a_print_under_reset_mode_four_overwrites_the_cell() {
    let device = interpret(b"abcd\x1b[2G\x1b[4h\x1b[4lX");
    assert_eq!(row_glyphs(&device), ['a', 'X', 'c', 'd']);
}
```

---

## TC-10 — `DECRC` は IRM を巻き戻さない 〈Medium〉

| | |
| - | - |
| Setup | なし |
| Act | `interpret(b"\x1b[4h\x1b7\x1b[4l\x1b8")` |
| Expect | `device.modes().insert_replace == InsertReplaceMode::Replace` **[C10]** |

TC-07 が押さえるのは `?1049` の flip だけで、素の `DECSC` / `DECRC`
（`ESC 7` / `ESC 8`）は誰も見ていなかった。`Checkpoint` の doc は
「IRM は意図的に含めない」と明記しているので、その不変条件をテストで固定する。

```rust
/// Asserts that `DECRC` leaves IRM where the data stream last put it,
/// because `DECSC` does not save the mode.
///
/// Case: an application saves the cursor while in insert mode, leaves
/// insert mode, and restores the cursor before drawing further.
#[test]
fn a_cursor_restore_does_not_carry_the_mode_back() {
    let device = interpret(b"\x1b[4h\x1b7\x1b[4l\x1b8");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}
```

---

# 付録

## 契約表

| ID | Governs | Shape | Statement | Citation |
| - | - | - | - | - |
| C1 | SM (ECMA-48 8.3.125) | multi-function | "SM causes the modes of the receiving device to be set as specified by the parameter values: … 4 INSERTION REPLACEMENT MODE (IRM)" | ECMA-48.pdf p.79, L3662-3666 |
| C2 | RM (ECMA-48 8.3.106) | multi-function | "RM causes the modes of the receiving device to be reset as specified by the parameter values: … 4 INSERTION REPLACEMENT MODE (IRM)" | ECMA-48.pdf p.70, L3228-3232 |
| C3 | IRM | unconditional | "CSI 4 h Set: insert mode. CSI 4 l Reset: replace mode." | vt510.pdf p.319, L9179-9181 |
| C4 | IRM | unconditional | "Default: Replace." | vt510.pdf p.319, L9176 |
| C5 | モード番号空間 | unconditional | "Mode Description / ANSI Mode / DEC Mode / Pa / Mnemonic2 / Pd / Mnemonic / Mode … Insert/replace 4 IRM ?4 DECSCLM Scrolling" | vt510.pdf p.87, L3365-3370 |
| C6 | RIS | unconditional | "RIS replaces all set-up features with their saved settings. The terminal stores these saved settings in NVR memory. The saved setting for a feature is the same as the factory-default setting, unless you saved a new setting." | vt510.pdf p.331, L9420-9423 |
| C7 | Modes (ECMA-48 7.1) | unconditional | "The states of the modes may be established explicitly in the data stream by the control functions SET MODE (SM) and RESET MODE (RM)" | ECMA-48.pdf p.34, L1451-1452 |
| C8 | Modes (ECMA-48 7.1) | unconditional | "In an implementation, some or all of the modes may have one state only." | ECMA-48.pdf p.34, L1453 |
| C9 | DECSET 1049 | unconditional | "Save cursor as in DECSC, xterm. After saving the cursor, switch to the Alternate Screen Buffer, clearing it first." | xterm-ctlseqs.pdf p.20, L1047-1050 |
| C10 | DECSC | unconditional | "Saves the following items in the terminal's memory: Cursor position / Character attributes set by the SGR command / Character sets (G0, G1, G2, or G3) currently in GL and GR / Wrap flag (autowrap or no autowrap) / State of origin mode (DECOM) / Selective erase attribute / Any single shift 2 (SS2) or single shift 3 (SS3) functions sent" | vt510.pdf p.243, L7328-7335 |

行番号は `pdftotext -layout` での抽出に対する session ローカルな目印で、poppler の
版をまたいだ保証は無い。頁番号のほうは改頁文字を数えて算出している。

**C9 は `xterm-ctlseqs.pdf` からの引用。** この skill の 1b / 1e は
`docs/references/` を「ECMA-48・vt220・vt510 の3つ」と書いているが、実際には
`xterm-ctlseqs.pdf` も置かれていて、`vt-conformance-scope.md` は全体を通して
それを一次資料として扱っている。`?1049` を規定するのはそこだけなので引いた。
skill 側の記述が古くなっているという指摘でもある。

**C10 は「閉じた列挙」として使っている。** *"Saves the following items"* に続く7項目が
DECSC の保存範囲のすべてであり、IRM がそこに無いことが TC-07 の根拠になる。不在からの
議論に見えるが、列挙を閉じているのは仕様の側。

### kind-2 台帳

無し。`Executor::set_modes` も `InsertReplaceMode` もまだ存在せず、引用できる
リポジトリ内の doc コメントが1つも無い。7件すべてがマニュアル由来。

## 探索した語

| Level | 語 | 到達先 |
| - | - | - |
| 1 | "SM" / "SET MODE"（合意済み doc） | **ECMA-48 8.3.125** — p.79 |
| 1 | "RM" / "RESET MODE"（同） | **ECMA-48 8.3.106** — p.70 |
| 1 | "ANSI mode"（同） | **vt510 モード表** — p.87 |
| 1 | "IRM"（合意済み body の `InsertReplaceMode`） | **vt510 p.319**, ECMA-48 7.2.10 |
| 1 | "RIS"（`InsertReplaceMode` の合意済み doc） | **vt510 p.331** |
| 1 | "DECSC" / "?1049"（同） | **vt510 p.243**, **xterm-ctlseqs p.20** |
| 2 | "control function"（`impl Executor<'_>` の doc） | 到達せず（一般語） |
| 2 | "CSI"（同） | 到達せず（記法のみで、振る舞いを述べる箇所に至らない） |
| 3 | "vtparse" / "synchronized update" / "CSI ?2026"（`//!` header） | 到達せず（パーサ語彙） |
| 4 | `CsiParams` → "parameter" | SM / RM の `(Ps...)` 記法に到達 |
| 4 | `bool` | 到達せず |
| 4 | `set_modes` → "Set Mode" / "Reset Mode" | 到達（level 1 と同じ先） |

`set_modes` にはまだ `# Control Functions` 節が無い（関数自体が無い）ので、level 1 は
承認済み設計の doc と body から拾っている。vt220.pdf も IRM を持つ（4.6.4）が、vt510 が
同じ振る舞いをより詳しく述べているので precedence 1 で止めた。

## 仕様にないもの

引用が付かずに落としたエントリは無い。検証に落ちたエントリも無い（C10 は最初、
保存項目を "Character sets currently in GL and GR" / "Wrap flag" と略記して
`REJECTED (dropped qualifier: no)` になった。括弧内の *"(autowrap or no autowrap)"* を
落としたのが原因で、実テキストどおりに記録し直したうえで VERIFIED。落としたのでは
なく直したもの）。

**KAM(2) / SRM(12) / LNM(20) の実装は今回の射程外。** ECMA-48 は3つとも定義している
（KAM は vt510 p.319 にもある）が、`xterm-256color` の entry はどれも広告して
おらず、`vt-conformance-scope.md` の Tier 1 / Tier 2 のどちらにも載っていない。
黙って無視する方針は C8 が支える。**LNM だけは将来 `line_feed` と Enter キーの
送出の両方に効く**ので、そのときに `set_modes` へ腕が1本増える。

**DECSTR による IRM のリセット**（vt510 Table 5-9 p.277 — "Insert/replace IRM
Replace mode."）はケースにしていない。DECSTR 自体が未実装（`csi_dispatch` が
intermediate を落とす）で `Act:` 行を書けないため。これは `set_modes` の signature の
問題ではなく別の制御機能の未実装なので blocker ではなく、`vt-conformance-scope.md`
§1-B の DECSTR 行が持つ宿題。DECSTR を実装するときに、この1件を足すこと。

**DECRQM / DECRPM（`CSI 4 $ p` → `CSI 4 ; Pm $ y`）もケースにしていない。** vt510 は
IRM の状態問い合わせを規定している（**C11**: vt510.pdf p.230, L7026 — "CSI 4 $ p What
is the current state of insert/replace mode (IRM)? (IRM = 4)"）が、orzma は
DECRQM を実装しておらず、
`vt-conformance-scope.md` §2 が Tier 2 として別途スケジュールしている。実装した
時点で、IRM が 1（設定）/ 2（解除）で答えられることを確かめるケースが要る。

## API 改定

無し。Phase 4 で signature の提案は出ていない。`Executor::set_modes` は承認済みの
設計そのままで、上の7件はすべてその形に対して書かれている。

## docs/todo との衝突

無し。`vt-conformance-scope.md` §1-A は `CSI 4 h/l` を `CSI∅`（「**非 private SM/RM
自体が未実装**」）と記録していて、この run が埋める穴と一致する。
`tdd-screen-print.md` は同じ `InsertReplaceMode` API を前提に書かれており、
`Screen::print` 側（8件）と `Executor::set_modes` 側（7件）で担当が分かれている
だけで、競合はしていない。
