# Test cases: Executor::set_private_modes（DECTCEM / `CSI ?25 h·l`）

対象は `crates/orzma_vt/src/interpreter.rs:587` の `Executor::set_private_modes`
のうち、DEC private mode 25（DECTCEM）が担う部分。参照したのは
`docs/references/` の `vt510.pdf` / `vt220.pdf` / `ECMA-48.pdf` /
`xterm-ctlseqs.pdf` の 4 冊で、citation は 21/21 検証済み（マニュアル 16、
リポジトリ内の明文契約 5）。

Phase 4 の結果: ケース一覧は承認、signature 提案は **案 A（デバイス全体に置く）**
が採択された。

> **以下の Rust は現状の木に対してコンパイルできない。** 案 A の
> `VtModes::text_cursor_enable` と `Screen::cursor` の新しい引数を前提にしている
> ため、まずその変更を入れてから転記する。またこのドキュメントの Rust は
> **一度もビルドしていない** — 貼って走らせられる完成品ではなく、転記のための
> 提案として読むこと。

このリストは Phase 4 で承認されたあと、`/forte:spec-review`（Codex ＋ Claude Code
Agent の並列レビュー）の指摘を受けて更新されている。**TC-A4 は承認後に追加された
ケース**で、著者の指示で反映した。TC-A2 のタグ・ベクタ・根拠と TC-04 のベクタ、
TC-06 の `Case:` も同レビューで訂正した。仕様由来の TC-01〜TC-07 は変更していない。

このリストを最初に書いたのは書き手だけで、citation の検証も自己検証である。
信用する前に 1〜2 本は自分で当たってほしい。

## 前提となる API 変更（案 A）

```rust
// crates/orzma_vt/src/device/modes.rs

/// Whether DECTCEM (DECSET 25) has the text cursor enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextCursorEnable {
    /// `CSI ? 25 h`: the cursor is drawn. The power-up default.
    #[default]
    Shown,
    /// `CSI ? 25 l`: the cursor is not drawn.
    Hidden,
}

impl TextCursorEnable {
    /// The state `DECSET 25` selects when set and `DECRST 25` when reset.
    pub fn from_decset(enabled: bool) -> Self { … }
}

// VtModes に足すフィールド:
    /// DECTCEM (DECSET 25): whether the text cursor is drawn.
    ///
    /// The device carries this rather than either screen, so a switch to
    /// the alternate screen keeps the state the application set.
    pub text_cursor_enable: TextCursorEnable,
```

```rust
// crates/orzma_vt/src/screen.rs — 引数が 1 つ増える
pub fn cursor(&self, text_cursor_enable: TextCursorEnable) -> Cursor
```

```rust
// crates/orzma_vt/src/device.rs — 委譲する新メソッド
/// The cursor an emitted frame carries: the active screen's write
/// position with the device's DECTCEM state folded in.
pub fn cursor(&self) -> Cursor {
    self.active_screen().cursor(self.modes.text_cursor_enable)
}
```

あわせて必要になるもの（spec §4-1 / §4-3 参照）:

- `screen.rs` の `use` ブロックへ `use crate::device::modes::TextCursorEnable;`
  （現在 `crate::device` からの import が 1 つも無い）
- prelude（`lib.rs:35`）へ `TextCursorEnable` を re-export
- `VtModes` / `Vt::modes` / `DeviceState::modes` の「input-relevant」doc の修正

`bool` ではなく enum にするのは好みではない。素の `bool` だと `VtModes` の
derive した `Default` が `false` = 不可視になり、TC-03 と TC-07 が両方落ちる。
`#[default] Shown` の enum なら derive のまま C3 を満たし、同ファイルの
`KeypadMode` / `ScreenKind` の書き方にも揃う。

同じ change-set に入る呼び出し側の修正（5 箇所、すべてクレート内。`Screen` は
`mod screen;` が private で prelude にも無いため外部破壊は無い）:

| 箇所 | 変更 |
| --- | --- |
| `frame.rs:141` | `cursor: device.cursor()`（`screen` の束縛は他のフィールドのため残る） |
| `interpreter.rs:59` | `let cursor_before = device.cursor();` |
| `interpreter.rs:68` | `!= executor.device.cursor()` |
| `frame.rs:286` | `#[cfg(test)]` 内の呼び出し |
| `screen/tests/cursor.rs:20` | 既存 1 件のテストの呼び出し |
| `screen.rs:1026` | `// TODO:` を DECSCUSR のみに縮小 |

`interpreter.rs:59`/`:68` の修正は TC-A1 が直接押さえる。ここを直し忘れると、
モードは正しく保存されるのに chunk 生存判定がそれを見ず、カーソルが画面から
消えない。

## テストケース一覧

High → Medium → Low の順に並べてある。High で止めても、仕様が明言している
挙動はすべて揃う。

| # | 名前 | Source | 優先度 |
| --- | --- | --- | --- |
| TC-01 | `a_dectcem_reset_hides_the_cursor` | C2 | High |
| TC-02 | `a_dectcem_set_shows_a_hidden_cursor` | C1 | High |
| TC-03 | `a_fresh_terminal_reports_a_visible_cursor` | C3 | High |
| TC-04 | `a_dectcem_reset_inside_a_multi_mode_list_is_applied` | C6, PL | High |
| TC-05 | `an_ansi_mode_reset_of_twenty_five_does_not_hide_the_cursor` | C10, C11 | High |
| TC-06 | `a_cursor_checkpoint_leaves_cursor_visibility_alone` | C5 | Medium |
| TC-07 | `a_reset_to_initial_state_restores_the_visible_cursor` | C9, C3, RS | Medium |
| TC-A1 | `hiding_the_cursor_makes_the_chunk_frame_relevant` | DM, CS, CV | Low |
| TC-A2 | `a_dectcem_write_that_changes_nothing_does_not_make_the_chunk_live` | DM | Low |
| TC-A3 | `the_alternate_screen_keeps_the_cursor_visibility` | TCE | Low |
| TC-A4 | `an_emitted_frame_carries_the_hidden_cursor` | CS, CV | Low |

Source タグ — **C1**: vt510 p.282 L8317（Set → visible）／**C2**: vt510 p.282
L8319（Reset → invisible）／**C3**: vt510 p.282 L8313-8314（Default: Visible）
／**C5**: vt510 p.243 L7328-7335（DECSC の保存項目列挙）／**C6**: vt510 p.345
L9758-9760（1 シーケンスに複数 Pd）／**C9**: vt510 p.331 L9438（RIS）／
**C10**: vt510 p.345 L9749-9751（ANSI と DEC を混ぜられない）／**C11**: vt510
p.87 L3365-3366・L3385（25 は DEC 側 Pd のみ）／**CS**: `frame.rs:217`
`Carried`／**CV**: `screen/cursor.rs:22` `Cursor::visible`／**DM**:
`lib.rs:240` `InterpretOutput::damaged`／**PL**: `interpreter.rs:587`
`set_private_modes`／**RS**: `device.rs:148` `DeviceState::reset`／**TCE**:
Phase 4 で合意した `VtModes::text_cursor_enable` の doc（未執筆）。全エントリは
付録の契約表と kind-2 台帳にある。

TC-01 から TC-07 までは仕様が述べる挙動。`TC-A` 番号の 4 件は仕様に対応する
文が無く、orzma 自身の frame 契約と、Phase 4 で合意した配置の決定だけを
根拠にしている。

テストコードは **`crates/orzma_vt/src/interpreter/tests/text_cursor_enable.rs`
（新規）** へ置き、`crates/orzma_vt/src/interpreter/tests.rs` に
`mod text_cursor_enable;` を足す。既存 24 モジュールが「制御機能ごと」に
分かれている慣習に従った。宣言はアルファベット順なので `tabulation` と `title` の
間に入れる。ヘルパは既存の `interpret()` / `damage_of()` / `liveness_after()` /
`Session::frame()` をそのまま使う。

モジュール冒頭に置く共通部分:

```rust
//! Tests for DECTCEM, the private mode that decides whether the text
//! cursor is drawn.

use super::*;

/// The visibility an emitted frame's cursor snapshot carries.
fn cursor_visible(device: &DeviceState) -> bool {
    device.cursor().visible
}
```

`DeviceState::cursor()` を経由するのが要点。モードと画面をヘルパ側で組み直すと、
それは本番の畳み込みの*再実装*になり、`frame.rs:141` の配線ミスをテストが
まるごと見逃す。TC-A4 はその穴を別経路から塞ぐが、ヘルパを本番と同じ 1 本に
しておくのが第一防衛線になる。

## TC-01 — `CSI ?25 l` はカーソルを不可視にする

| | |
| - | - |
| Setup | 新規端末（`interpret`） |
| Act | `interpret(b"\x1b[?25l")` |
| Expect | カーソルが不可視になる **[C2]** |

DECTCEM の片側そのもの。`vt-conformance-scope.md` の実測で 1 セッション
590 回と最頻出であり、これが効かないと再描画中もカーソルが本文の上に
残り続ける。

```rust
/// Asserts that `CSI ? 25 l` makes the cursor invisible.
///
/// Case: nvim hides the caret before it repaints a pane, so the block
/// does not sit on top of the text while the rows are rewritten.
#[test]
fn a_dectcem_reset_hides_the_cursor() {
    let device = interpret(b"\x1b[?25l");
    assert!(!cursor_visible(&device));
}
```

## TC-02 — `CSI ?25 h` は隠れたカーソルを再び可視にする

| | |
| - | - |
| Setup | 新規端末 |
| Act | `interpret(b"\x1b[?25l\x1b[?25h")` |
| Expect | カーソルが可視になる **[C1]** |

新規端末は C3 により既に可視なので、`?25h` を単独で送っても空振りする。
先に隠しておくことが、この 1 行が実際に何かを変えたと言える唯一の形になる。

```rust
/// Asserts that `CSI ? 25 h` makes a hidden cursor visible again.
///
/// Case: nvim finishes the repaint and brings the caret back so the
/// user can see where the next keystroke will land.
#[test]
fn a_dectcem_set_shows_a_hidden_cursor() {
    let device = interpret(b"\x1b[?25l\x1b[?25h");
    assert!(cursor_visible(&device));
}
```

## TC-03 — DECTCEM を一度も受け取っていない端末は可視カーソルを報告する

| | |
| - | - |
| Setup | 新規端末 |
| Act | `interpret(b"$ ")` |
| Expect | カーソルが可視 **[C3]** |

C3 の "Default: Visible" を押さえる。`VtModes` の `Default` 実装を直接突く
ケースで、可視性を素の `bool` として足すと derive した `Default` が
`false` になり、起動直後からカーソルが消える。

`crates/orzma_vt/src/screen/tests/cursor.rs` の
`the_cursor_reports_the_write_position_and_is_visible` が `Screen` 層で
似た内容を押さえているが、あちらは `Screen::cursor()` が返す固定値を見て
いるだけで、DECTCEM の既定値を通っていない。案 A では両者が見る値が
別になるので、重複ではなく層が違う。

```rust
/// Asserts that a terminal that has seen no DECTCEM reports a visible
/// cursor.
///
/// Case: a shell prints its prompt on a freshly spawned terminal, and
/// the user has to see the caret waiting after it.
#[test]
fn a_fresh_terminal_reports_a_visible_cursor() {
    let device = interpret(b"$ ");
    assert!(cursor_visible(&device));
}
```

## TC-04 — 複数モード列の中の `?25` が適用される

| | |
| - | - |
| Setup | 先行 chunk の `CSI ?1004h` で `focus_in_out` を true にしておく |
| Act (a) | `interpret(b"\x1b[?1004h\x1b[?25;9999;1004l")` |
| Expect (a) | カーソルが不可視になる **[C6]** ／ `modes().focus_in_out == false` **[PL]** |
| Act (b) | `interpret(b"\x1b[?25l\x1b[?12;25h")` |
| Expect (b) | カーソルが可視になる **[C6]** |

C6 は 1 シーケンスに複数の Pd を置けると明言している。9999 を orzma が
実装する 2 つの番号の**間**に挟むのが要点で、未知の番号で `for` を抜ける
実装と、`?25` が単独パラメータのときだけ届く実装の両方を落とす。
`focus_in_out` を先に立てておかないと reset 後の false が既定値と区別
できず、後半のアサーションが空振りする。

(b) は合成ではなく**実機の terminfo が実際に送る列**。このマシンの
`infocmp -1 xterm-256color` は `civis=\E[?25l` / `cnorm=\E[?12l\E[?25h` /
`cvvis=\E[?12;25h` を広告しており、複数 Pd の DECTCEM は `cvvis` として日常的に
届く。`?12`（att610 blink）は未実装で `set_mouse_mode` に落ちて no-op になるが、
それが `?25` を潰さないことをここで固定する。`?12` は `25` の**前**に来るので
「実装済みの番号の間に未知の番号を挟む」性質は (a) にしか無く、両方残す。

```rust
/// Asserts that `? 25` inside a multi-mode list is applied, and that an
/// unimplemented number ahead of a later implemented one hides neither.
///
/// Case: an application shuts down and turns off cursor visibility, a
/// mode this terminal does not implement, and focus reporting in one
/// `CSI ? Pm l`.
#[test]
fn a_dectcem_reset_inside_a_multi_mode_list_is_applied() {
    let device = interpret(b"\x1b[?1004h\x1b[?25;9999;1004l");
    assert!(!cursor_visible(&device));
    assert!(!device.modes().focus_in_out);

    let through_cvvis = interpret(b"\x1b[?25l\x1b[?12;25h");
    assert!(cursor_visible(&through_cvvis));
}
```

## TC-05 — `?` の無い `CSI 25 l` はカーソルを隠さない

| | |
| - | - |
| Setup | 新規端末 |
| Act | `interpret(b"\x1b[25l")` |
| Expect | カーソルは可視のまま **[C10] [C11]** |

C11 のとおり 25 は DEC 側 Pd にしか存在せず、C10 は 1 つの SM/RM が ANSI
空間と DEC 空間をまたげないと述べる。**現状この列は `csi_dispatch` 末尾の
`_ => {}` に落ちるので、このテストは今は自明に通る。** それでも入れる価値が
あるのは、`vt-conformance-scope.md` §1-A の次項が IRM（`CSI 4 h/l`）— つまり
非 private SM/RM ハンドラの新設 — だからで、そのハンドラが 25 まで拾って
しまう回帰への番人になる。

```rust
/// Asserts that the ANSI-mode spelling `CSI 25 l`, which carries no
/// `?`, leaves the cursor visible.
///
/// Case: a program drives the terminal through non-private SM and RM,
/// and one of the numbers in its parameter list happens to be 25.
#[test]
fn an_ansi_mode_reset_of_twenty_five_does_not_hide_the_cursor() {
    let device = interpret(b"\x1b[25l");
    assert!(cursor_visible(&device));
}
```

## TC-06 — DECSC/DECRC は可視性を保存も復元もしない

| | |
| - | - |
| Setup | 新規端末 |
| Act (a) | `interpret(b"\x1b[?25l\x1b[?1048h\x1b[?25h\x1b[?1048l")` |
| Expect (a) | カーソルは可視のまま **[C5]** |
| Act (b) | `interpret(b"\x1b[?1048h\x1b[?25l\x1b[?1048l")` |
| Expect (b) | カーソルは不可視のまま **[C5]** |
| Act (c) | `interpret(b"\x1b[?25l\x1b7\x1b[?25h\x1b8")` |
| Expect (c) | カーソルは可視のまま **[C5]** |

C5 は DECSC の保存項目を列挙形式で述べており、可視性はそこに無い。
(a) は「チェックポイントがビットを保存してしまった」実装を、(b) は
「復元が既定値へ戻してしまった」実装を落とす。`?1048` が本メソッドを通る
綴りなので主、`ESC 7`/`ESC 8` は同じ契約を ESC 経路で押さえる従。

```rust
/// Asserts that DECSC and DECRC neither save nor restore cursor
/// visibility, in the `CSI ? 1048` spelling and the `ESC 7` / `ESC 8`
/// one alike.
///
/// Case: a full-screen application checkpoints and restores the cursor
/// around a redraw, sometimes with the caret hidden when it saves and
/// sometimes when it restores, and reaches the checkpoint through the
/// private-mode spelling on one path and `ESC 7` / `ESC 8` on another.
#[test]
fn a_cursor_checkpoint_leaves_cursor_visibility_alone() {
    let restored_while_visible = interpret(b"\x1b[?25l\x1b[?1048h\x1b[?25h\x1b[?1048l");
    assert!(cursor_visible(&restored_while_visible));

    let restored_while_hidden = interpret(b"\x1b[?1048h\x1b[?25l\x1b[?1048l");
    assert!(!cursor_visible(&restored_while_hidden));

    let restored_through_esc = interpret(b"\x1b[?25l\x1b7\x1b[?25h\x1b8");
    assert!(cursor_visible(&restored_through_esc));
}
```

## TC-07 — RIS は既定値の可視カーソルへ戻す

| | |
| - | - |
| Setup | 新規端末 |
| Act | `interpret(b"\x1b[?25l\x1bc")` |
| Expect | カーソルが可視に戻る **[C9] [C3] [RS]** |

RS が「すべてのモードを電源投入時の状態へ戻す」と述べ、C3 がその状態を
Visible と名指す（C9 は同じことのマニュアル側の言い方）。TC-03 と同じく
`Default` 実装を突くが、経路が違う: こちらは `DeviceState::reset` の
`self.modes = VtModes::default()` を通る。

```rust
/// Asserts that `RIS` returns DECTCEM to its visible default.
///
/// Case: a program leaves the caret hidden when it dies, and the shell
/// issues a hard reset to get a usable terminal back.
#[test]
fn a_reset_to_initial_state_restores_the_visible_cursor() {
    let device = interpret(b"\x1b[?25l\x1bc");
    assert!(cursor_visible(&device));
}
```

## TC-A1 — カーソルを隠した chunk は frame-relevant になる

| | |
| - | - |
| Setup | 新規端末 |
| Act | `damage_of(b"\x1b[?25l")` |
| Expect | `true` **[DM] [CS] [CV]** |

仕様側に「再描画せよ」と述べた文は無く、これは orzma 自身の frame 契約。
CV が DECTCEM を `Cursor` の中に置き、CS が `Cursor` を含む `Carried` の
変化は frame を負うと述べ、DM がそれを chunk 単位の `damaged` に落とす。

このケースが要るのは、状態を正しく保存してなお画面に届かない実装が
あり得るから。`Interpreter::parse` の chunk 生存判定はカーソルをアクティブ
画面から読むので、デバイス全体に置いたフィールドは `interpreter.rs:59`/`:68`
の読み取りを直さない限り判定に映らない。

```rust
/// Asserts that a chunk hiding the cursor is reported as
/// frame-relevant.
///
/// Case: nvim sends nothing but `CSI ? 25 l`, and the host has to open
/// a coalesce window for the caret to actually leave the screen.
#[test]
fn hiding_the_cursor_makes_the_chunk_frame_relevant() {
    assert!(damage_of(b"\x1b[?25l"));
}
```

## TC-A2 — 何も変えない DECTCEM は chunk を live にしない

| | |
| - | - |
| Setup | 先行 chunk `\x1b[?25l` でカーソルを隠しておく |
| Act | `liveness_after(b"\x1b[?25l", b"\x1b[?25l")` |
| Expect | `false` **[DM]** |

DM は `damaged` を「この chunk が frame-relevant な何かを生んだか」と定義して
おり、何も変えない代入はそれに当たらない。判定は `interpreter.rs:68` の
カーソル差分で、`Carried` の比較（`frame.rs`）ではない — **`damage_of` が読むのは
`InterpretOutput.damaged` であって emit の可否ではない**ので、タグは `CS` ではなく
`DM` になる。chunk が live でも emit が `None` を返すことはあり、この 2 つは別の層。

**ベクタが `Hidden → Hidden` なのが要点。** 新規端末への `?25h` は `Shown → Shown`
なので、実装前の今でも、書き込みのたびに `Shown` をハードコードする実装でも通って
しまう。`?25l` を 2 回送る形なら、2 回目が no-op であるためにモードが chunk を
またいで Hidden のまま保たれている必要があり、永続性と no-op 性を同時に固定できる。

nvim の DECTCEM 590 回（`vt-conformance-scope.md`）は hide / show の交互なので、
**うち約半分は本当に可視性が変わる**。この実装後にその分 `damaged` が増えるのは
正しい挙動であって、このケースが防ぐものではない。ここが守るのは同じ値の
再宣言だけ。

```rust
/// Asserts that a DECTCEM write that changes nothing leaves the chunk
/// out of the frame-relevant set.
///
/// Case: an application re-asserts the cursor visibility it already has
/// as part of the escape sequence it emits on every redraw.
#[test]
fn a_dectcem_write_that_changes_nothing_does_not_make_the_chunk_live() {
    assert!(!liveness_after(b"\x1b[?25l", b"\x1b[?25l"));
}
```

## TC-A3 — 代替画面へ切り替えても DECTCEM が保たれる

| | |
| - | - |
| Setup | 新規端末 |
| Act | `interpret(b"\x1b[?25l\x1b[?1049h")` |
| Expect | 代替画面でもカーソルは不可視のまま **[TCE]** |

Phase 4 で採択された案 A の配置そのものを押さえる。根拠は
`VtModes::text_cursor_enable` に書く doc の一文
（"The device carries this rather than either screen, so a switch to the
alternate screen keeps the state the application set."）で、これは**まだ
木に存在しない** — 実装時にこの文を書いて初めて台帳エントリとして成立する。

案 B（画面ごとに置く）を採ればこのケースは反転し、代替画面でカーソルが
可視に戻る。nvim は起動時に `?25l` → `?1049h` の順で送るので、反転側では
代替画面に入った瞬間にカーソルが戻ってしまう。

```rust
/// Asserts that switching to the alternate screen keeps the DECTCEM
/// state the application set.
///
/// Case: nvim hides the caret and then enters the alternate screen as
/// it starts up, and the caret stays hidden until nvim asks for it back.
#[test]
fn the_alternate_screen_keeps_the_cursor_visibility() {
    let device = interpret(b"\x1b[?25l\x1b[?1049h");
    assert!(!cursor_visible(&device));
}
```

## TC-A4 — emit された frame が不可視カーソルを運ぶ

| | |
| - | - |
| Setup | `Session::new()` を作り、最初の bootstrap frame を排出しておく |
| Act | `\x1b[?25l` を流し、`session.frame()` を読む |
| Expect | 返った `Frame` の `cursor.visible == false` **[CS] [CV]** |

**このケースだけが本番の畳み込みを通る。** 他の 10 件はすべてヘルパ
`cursor_visible()` を経由し、それは frame 組み立て（`frame.rs:141`）と同じことを
テスト側で行う。したがって `frame.rs:141` が `TextCursorEnable::Shown` をリテラルで
渡す実装でも 10 件はすべて通り、それでいてカーソルは画面から消えない。CS が
「`Carried` の各セクションの変化は frame を負う」と述べ、CV が可視性を `Cursor` の
中に置くので、契約としては emit された `Frame` を読むところまでが本来の要求になる。

`Session`（`interpreter/tests.rs:57`）は chunk をまたいで 1 つの端末を保つ rig で、
`frame()` は `:74` にある。bootstrap frame を先に排出しておかないと、最初の
`frame()` が初期描画を返してしまい `?25l` の効果と区別できない。

```rust
/// Asserts that the frame a chunk emits carries the hidden cursor, not
/// just the device state a test can read back.
///
/// Case: nvim hides the caret and the host repaints from the frame it
/// receives, which is the only cursor the screen ever shows.
#[test]
fn an_emitted_frame_carries_the_hidden_cursor() {
    let mut session = Session::new();
    let _ = session.feed(b"$ ");
    let _ = session.frame();

    let _ = session.feed(b"\x1b[?25l");
    let frame = session.frame().expect("hiding the cursor owes a frame");
    assert!(!frame.cursor.visible);
}
```

> このケースは Phase 4 の承認後に `/forte:spec-review` の指摘を受けて追加した。
> 使う API は確認済み: `Session::feed(&[u8]) -> InterpretOutput`
> (`interpreter/tests.rs:69`)、`Session::frame() -> Option<Frame>` (`:74`)、
> `Frame::cursor: Cursor` (`frame.rs:44`)。

---

## 付録

### 契約表

| ID | Governs | Shape | Statement | Citation |
| --- | --- | --- | --- | --- |
| C1 | DECTCEM | unconditional | "Set: makes the cursor visible." | vt510.pdf p.282, L8317 |
| C2 | DECTCEM | unconditional | "Reset: makes the cursor invisible." | vt510.pdf p.282, L8319 |
| C3 | DECTCEM | unconditional | "This control function makes the cursor visible or invisible. Default: Visible" | vt510.pdf p.282, L8313-8314 |
| C4 | DECSTR | unconditional | "Text cursor enable / DECTCEM / Cursor enabled." | vt510.pdf p.277, L8203 |
| C5 | DECSC | 列挙（網羅） | "Saves the following items in the terminal's memory: Cursor position / Character attributes set by the SGR command / Character sets (G0, G1, G2, or G3) currently in GL and GR / Wrap flag (autowrap or no autowrap) / State of origin mode (DECOM) / Selective erase attribute / Any single shift 2 (SS2) or single shift 3 (SS3) functions sent" | vt510.pdf p.243, L7328-7335 |
| C6 | SM/RM (DEC 形式) | multi-function | "Pd indicates a DEC mode to set. Table 5-8 lists the Pd values for DEC modes. You can use more than one Pd value in a sequence." | vt510.pdf p.345, L9758-9760 |
| C7a | DECTCEM (VT220) | 傍証 | "Text cursor enable mode determines if the text cursor is visible." | vt220.pdf p.27, L1271 |
| C7b | DECTCEM (VT220) | 傍証 | "Set 9/11 3/15 3/2 3/5 6/8 Makes the cursor visible. CSI ? 2 5 h" | vt220.pdf p.27, L1276-1277 |
| C7c | DECTCEM (VT220) | 傍証 | "Reset 9/11 3/15 3/2 3/5 6/12 Makes the cursor not visible. CSI ? 2 5 l" | vt220.pdf p.27, L1279-1280 |
| C8a | DECTCEM (xterm) | 傍証 | "Ps = 2 5 ⇒ Show cursor (DECTCEM), VT220." | xterm-ctlseqs.pdf p.19, L960 |
| C8b | DECTCEM (xterm) | 傍証 | "Ps = 2 5 ⇒ Hide cursor (DECTCEM), VT220." | xterm-ctlseqs.pdf p.21, L1109 |
| C9 | RIS | unconditional | "Sets all features listed on set-up screens to their saved settings." | vt510.pdf p.331, L9438 |
| C10 | SM | unconditional | "You use the ANSI format to set one or more ANSI modes. You use the DEC format to set one or more DEC modes. You cannot set ANSI and DEC modes with the same SM sequence." | vt510.pdf p.345, L9749-9751 |
| C11a | Table 5-7 | unconditional | "Mode Description / ANSI Mode / DEC Mode / Pa / Mnemonic2 / Pd / Mnemonic / Mode" | vt510.pdf p.87, L3365-3366 |
| C11b | Table 5-7 | unconditional | "Text cursor enable ?25 DECTCEM" | vt510.pdf p.87, L3385 |
| C12 | `?1049` (xterm) | multi-function | "Ps = 1 0 4 9 ⇒ Save cursor as in DECSC, xterm. After saving the cursor, switch to the Alternate Screen Buffer, clearing it first. … This control combines the effects of the 1 0 4 7 and 1 0 4 8 modes." | xterm-ctlseqs.pdf p.20, L1047-1052 |

### kind-2 台帳

各エントリは `grep -F` で実在を確認済み。

```
CV — crates/orzma_vt/src/screen/cursor.rs:22, Cursor::visible
     "True when the application wants the cursor drawn — DECTCEM
      (`TermMode::SHOW_CURSOR`) and a non-Hidden DECSCUSR shape."
     justifies: DECTCEM の状態は Cursor::visible が報告する

DM — crates/orzma_vt/src/lib.rs:240, InterpretOutput::damaged
     "Whether this chunk produced anything frame-relevant — staged row
      damage, cursor motion, or a mutated frame-visible section — so the
      owner knows to open its coalesce window."
     justifies: カーソルスナップショットを変えた chunk は damaged を立てる

CS — crates/orzma_vt/src/frame.rs:214, Carried
     "The sections every frame carries unconditionally, compared as one
      value so a change in any of them owes a frame."
     justifies: Cursor が変われば frame を負い、変わらなければ負わない

RS — crates/orzma_vt/src/device.rs:148, DeviceState::reset
     "Returns both screens and every mode to their power-up state"
     （同 doc の `# Control Functions` に `RIS` (`ESC c`)）
     justifies: RIS が DECTCEM を電源投入時の状態へ戻す

PL — crates/orzma_vt/src/interpreter.rs:587, Executor::set_private_modes
     "A sequence may carry several modes at once, and an unimplemented
      one must not hide an implemented one later in the list."
     justifies: 未知の番号の隣でも ?25 は適用される

TCE — crates/orzma_vt/src/device/modes.rs, VtModes::text_cursor_enable
      "The device carries this rather than either screen, so a switch to
       the alternate screen keeps the state the application set."
      justifies: ?25l のあとの ?1049h でカーソルは不可視のまま
      ※ Phase 4 で合意した文であり、**まだ木に存在しない**。実装時に
        この doc を書いて初めて台帳エントリとして成立する
```

### 探索した語

| Level | 出所 | 語 | 結果 |
| --- | --- | --- | --- |
| 1 | メソッドの `///` | `DECSET` / `DECRST` | SM/RM の DEC 形式へ到達（C6, C10） |
| 2 | 囲う `impl` の doc | `CSI` / "control function" | 行動を述べる文に届かず |
| 3 | ファイルの `//!` | "synchronized update" / `CSI ?2026` | 本件と無関係 |
| 4 | signature と メソッド名 | "private mode" / "DEC mode" / `Pd` | Table 5-7 と SM/RM へ到達（C11） |
| 実行スコープ | 引数と `vt-conformance-scope.md` | `DECTCEM` / "Text Cursor Enable" / `?25` | 定義本体へ到達（C1-C3） |
| 契約から辿った語 | — | `DECSC` / `DECRC` / `DECSTR` / `RIS` / `SM` / `RM` / `?1048` / `?1049` | C4, C5, C9, C12 へ到達 |

4 冊横断の出現数（`grep -ic`、falsifiable にするための記録）:

| 語 | vt510 | vt220 | ECMA-48 | xterm-ctlseqs |
| --- | ---: | ---: | ---: | ---: |
| `DECTCEM` | 9 | 4 | **0** | 2 |
| `Text Cursor Enable` | 6 | 5 | **0** | 0 |
| `Show cursor` | 0 | 0 | 0 | 1 |
| `Hide cursor` | 0 | 0 | 0 | 1 |
| `cursor visible` | 2 | 1 | 0 | 0 |
| `cursor invisible` | 1 | 0 | 0 | 0 |

ECMA-48 は DECTCEM を 1 件も持たない。DEC private mode なので当然であり、
この語で ECMA-48 へ降りる意味は無い。

### 仕様にないもの

- **DECTCEM と代替画面の相互作用** — 4 冊のどれも直接は規定していない。DEC に
  あるのはページメモリであって代替画面ではなく、xterm の ctlseqs も `?25` の
  項は "Show cursor" / "Hide cursor" としか書かない。TC-A3 はこの空白を
  Phase 4 の決定（案 A）で埋めている。
  ただし C12 が `?1049` を「DECSC として保存し、代替画面へ切り替え、消す」の
  3 つの効果に尽きると述べており、C5 の保存項目列挙に可視性が無いことと
  合わせると、案 A 側を支持する間接的な根拠にはなる（下の「承認後の発見」）。
- **DECRQM/DECRPM によるモード 25 の問い合わせ応答** — `vt-conformance-scope.md`
  の Tier 2。DECRQM 自体が未実装のため本実行の対象外。

### API 改定

**採択: 案 A（デバイス全体に置く）。** 内容は冒頭「前提となる API 変更」のとおり。

- 解決したブロッカー: **O — unobservable effect**。C1/C2（vt510 p.282,
  L8317・L8319）が要求する `Expect: …cursor().visible == false` を、現行 API は
  書けなかった。`VtModes` にも `Screen` にもフィールドが無く、`Screen::cursor()`
  は `visible: true` を直書きして `&self` しか取らなかったため。
- 有効化されたケース: TC-01〜TC-07、TC-A1、TC-A2、TC-A3。

**持ち越し（本メソッドの管轄外）:**

- **C4 — DECSTR は DECTCEM を "Cursor enabled." へ戻す**（vt510 p.277, L8203）。
  DECSTR（`CSI ! p`）自体が未実装で、`vt-conformance-scope.md` §1-B の
  `INTER∅` に該当する。DECSTR を実装するときに
  `text_cursor_enable = TextCursorEnable::Shown` を含めること。この citation は
  検証済みなので、そのとき洗い直す必要は無い。

### 承認後の発見（remark）

Phase 5 の転記中に **C12**（xterm p.20, L1047-1052）を見つけた。`?1049` の
効果が「DECSC として保存 → 代替画面へ切替 → 消す」に尽き、それが `1047` と
`1048` の合成だと明言している。C5 の DECSC 保存項目に可視性が無いことと
合わせると、切替が可視性を保存も復元もしないという読みが立ち、TC-A3 は
設計判断だけでなく仕様側からも支持される。

ただし**ケース一覧は承認時点で閉じている**ため、TC-A3 のタグは承認された
`[TCE]` のままにしてある。`[C12]` を足すかどうかは著者の判断で、足すなら
それは別の実行になる。

### 出典なしの改善提案

いずれもゲートには出していない、単なる所見。

- `screen.rs:1026` の `// TODO:` は DECTCEM 実装後に DECSCUSR のみへ縮小する。
  現在は "once DECSCUSR and DECTCEM land" と両方を挙げている。
- `Screen::cursor()` に引数が増えると、テストごとに
  `device.active_screen().cursor(device.modes().text_cursor_enable)` と
  書くことになる。**この懸念は `DeviceState::cursor()` の採用で解消した** — ヘルパは
  `device.cursor().visible` の 1 行になり、本番の畳み込みを再実装しない。当初
  「定義が 2 箇所になる」として推奨しなかったのは、`Screen::cursor()` を**無変更のまま**
  `DeviceState` 側で可視性を当てる案のことで、採用したのは `Screen::cursor(mode)` へ
  **委譲する**形なので定義は 1 箇所のままになる。

### docs/todo との衝突

- **`vt-conformance-scope.md:45` と `:147` が `screen.rs:988` を指しているが、
  当該 `// TODO:` は現在 `screen.rs:1026` にある。** ECH / ICH / DCH が入った
  後に 16 行ぶんずれた。DECTCEM を実装するときに両方の行参照を更新すること。
- `vt-conformance-scope.md` §5 の実装順 3 は **DECAWM / DECTCEM / カーソル点滅 /
  DECSCUSR を 1 ステップにまとめ**、「DECSC/DECRC の保存範囲もここで揃える」と
  している。本実行は DECTCEM 単独を対象にした。なお C5 の保存項目列挙には
  "Wrap flag (autowrap or no autowrap)" があり **DECAWM は DECSC が保存する** の
  に対し、可視性は列挙に無く **DECTCEM は保存しない**（TC-06）。同じステップに
  居るが保存範囲の扱いは逆になるので、まとめて実装するなら分けて扱うこと。
  この解決は行わない — 一覧はあくまで提案として報告する。
