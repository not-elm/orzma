# マウスホイールのアプリ転送（nvim でスクロールできるようにする）

記録日: 2026-09-15 / 対象リビジョン: `182cacb3` / ブランチ: `vim-mouse-scroll`

## 1. 解決する問題

nvim でファイルを開くとマウスホイールでスクロールできない。

`src/input/mouse/wheel.rs` のホイールディスパッチャは、アプリのマウス追跡モードを一切
見ずに、常に `TerminalViewportScroll` → `RequestTtyScroll` としてローカルの
スクロールバックにだけ流している。モジュール冒頭に
`TODO: reintroduce app-forward wheel-reporting against orzma_tty.` が残っている通り、
アプリ転送はエンジン差し替え（#268）の時点で外されたままになっている。

nvim は既定の `mouse=nvi` で `DECSET 1002`（button-event tracking）と `DECSET 1006`
（SGR）を送ってくる。1000 は送らない（`mouse_move_enabled` のときだけ 1003 が加わる）。
ホイールレポート（`cb` 64 / 65）を PTY に書けばスクロールする。alt screen には
スクロールバックが無い（`crates/orzma_vt/src/device.rs:36-37` の doc、実体は `:43` の
`alternate: Screen::new(size, 0)`）ため、現状のローカル
スクロールは nvim 表示中は原理的に無反応になる。

### 既に存在するもの

| 部品 | 場所 |
|------|------|
| ホイールレポートのエンコーダ（SGR / X10、`WheelUp=64` … `WheelRight=67`） | `crates/orzma_tty/src/input/mouse.rs` |
| PTY への書き込み（生きているエンコーディングで符号化） | `OrzmaTty::send_mouse`（`crates/orzma_tty/src/lib.rs:296`） |
| バックエンドコマンド | `OrzmuxCommand::MouseInput`（`crates/orzmux/src/protocol.rs:148`） |
| Bevy リクエストイベント | `RequestTtyMouseInput`（`crates/bevy_orzmux/src/requests/mouse_input.rs`） |
| VT 側のモード状態 | `VtModes::mouse_tracking` / `mouse_encoding` / `alternate_scroll` / `alternate_scroll_active()`（`crates/orzma_vt/src/device/modes.rs`） |
| 旧ルータ（コメントアウトで保存されている） | `crates/orzma_tty/src/input/wheel.rs` 全体 |
| バースト上限の設定値（移植待ちで `#[expect(dead_code)]`） | `WheelConfig::max_protocol_events_per_frame`（`src/input/bindings.rs:57`） |

欠けているのは **GUI から `RequestTtyMouseInput` を呼ぶ経路**と、そこで分岐するための
**モード知識**だけである。

## 2. 決めたこと（要件）

1. マウス追跡が有効なとき、ホイールはアプリにレポートとして転送する。
2. **Shift は 1 段階下の経路へのフォールスルー**を意味する。「常にローカルスクロール」に
   すると alt screen に履歴が無いため nvim 内で無反応になる。Shift を「マウスレポートを
   送らない」だけの意味にすれば、nvim では alternate scroll 経由で矢印キーが飛び、通常
   画面の追跡アプリ（fzf 等）では端末のスクロールバックが動く。どこでも死に手にならない。
3. **alternate scroll（DECSET 1007）に対応し、既定を ON にする**。`less` / `man` の
   ように追跡を使わず alt screen を使うアプリでホイールが効くようになる。1007 を持った上で
   既定 ON なのは Alacritty（`TermMode::default()` に `ALTERNATE_SCROLL`）。kitty /
   WezTerm は 1007 を実装せず常に矢印キーへ変換する。xterm の `alternateScroll` 資源と
   iTerm2 の隠し default `AlternateMouseScroll` は既定 false だが、そちらには合わせない。
   kitty / WezTerm が無条件変換である以上、観測される挙動としては ON が多数派である。
4. スクロールバック表示中（`display_offset > 0`）の特別扱いはしない。追跡中なら転送する。
   帰結として、`OrzmaTty::send_mouse` は live tail にスナップしない
   （`crates/orzma_tty/src/lib.rs:294-299`）ので、履歴を遡った状態で送るレポートは
   ビューポート相対のセル座標を運び、アプリはそれを生画面の座標として読む。alt screen には
   履歴が無いので nvim では起こらず、通常画面で追跡するアプリ（fzf 等）でのみ到達する。
5. 今回の実装対象はホイールのみ。ボタン / ドラッグ転送は後続 PR に回すが、モード知識の
   置き場所はその PR がそのまま乗る形に決める。

## 3. 採用した approach と、採らなかったもの

**採用: GUI が判断する。** バックエンドがモード差分を GUI に通知し、ペイン entity の
コンポーネントに反映する。`wheel.rs` がそれを読んで純粋ルータを呼び、既存の 3 つの
リクエストイベントに振り分ける。

- 新しい protocol **コマンド**は増えない（増えるのはイベント 1 つ）。
- 転送するかローカル選択に使うかの判断は、GUI 側のジェスチャ状態機械（ドラッグの arm、
  クリック回数、カーソル形状）と競合する種類の判断なので、GUI 側にある方が素直。後続の
  ボタン PR は「追跡中はローカル選択のドラッグを arm しない」判断を必要とする。

**採らなかった A: バックエンドが判断する。** `OrzmuxCommand::Wheel` を 1 つ足し、
バックエンドが `vt.modes()` を見て適用する。今回の変更は最小で済み、モードのズレも無いが、
ボタン PR で結局モード伝播が必要になり、配管を後ろにずらすだけになる。

**採らなかった C: `orzma_vt::Frame` にモードを載せる。** 追跡を有効にする DECSET が画面を汚さない
場合に coalescer がフレームを出さず、GUI がモード変更を知れない穴がある。加えて
`Frame` の全構築箇所・`differs_from` / `apply` / `quiet_frame` とそのテスト群に波及する。

## 4. モード伝播

```
PTY 出力 → OrzmaVt (DECSET 1002 / 1006 / 1007)
  └ orzmux backend: emit_pump_output で Pane::last_modes と比較 → 変化時のみ
      OrzmuxEvent::Modes { pane, modes: VtModes }
        └ bevy_orzmux::drain → TtyModesSignal { terminal, modes }
             └ TtyModesPlugin (crates/bevy_orzmux/src/modes.rs)
                  └ コンポーネント TtyModes(pub VtModes) を「異なるときだけ」書く
```

- `OrzmuxPane` に `#[require(TtyModes)]` を足す。`TtyTitle`（`crates/bevy_orzmux/src/title.rs:15-38`）
  が完全な前例で、`a_pane_requires_a_title`（同 `:140-144`）はそのまま写せる。Bevy 0.19 の
  裸の `#[require(T)]` が要求するのは `T: Default` だけなので、
  `#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)] struct TtyModes(pub VtModes)`
  で足りる（`set_if_neq` が要る `PartialEq` も同時に満たす）。
- バックエンド側は `Pane`（`crates/orzmux/src/backend/pane.rs:11-19`）に
  `last_modes: VtModes` を足し、ペイン生成時に `tty.vt().modes()` で初期化する。比較は
  `pump_pane` の後ではなく **`emit_pump_output` の中**で行う。`pump_pane` は `PUMP_ROUNDS`
  回ループし return が 3 箇所あり、うち `ChildExit` → `close_pane` の経路はループ後の
  コードに到達しないので、ループ後に置くと取りこぼす。`emit_pump_output` を選ぶ理由は
  「ペインごとのイベントを出す唯一の漏斗だから」ではない（`publish_layout` も `flush_now` の
  後に `Signal` / `Frame` をインラインで emit している、`crates/orzmux/src/backend.rs:411-429`）。
  正しい理由は、**`tty.pump()` の直後に走る唯一の場所**＝PTY バイトが解釈される唯一の場所で
  あり、`resize` も `flush_now` も `VtModes` を変えられないからである。
- 1 回の `pump_pane` は `emit_pump_output` を最大 `PUMP_ROUNDS`（= 4、`backend.rs:586`）回
  呼ぶので `Modes` も最大 4 回出うる。合流するのは 1 ラウンド内の遷移だけで、呼び出し単位で
  1 イベントに畳む保証は無い。下流は `set_if_neq` が吸収するので無害。
- 初期値 `VtModes::default()` は新品の `OrzmaVt` の状態と一致するので、`PaneOpened` 時の
  初回通知は不要。シェルが最初に送る DECSET は pump 経由の差分で拾える。§7 で
  `alternate_scroll` の既定を反転させても、両者が同じ `VtModes::default()` を通る限り
  この不変条件は保たれる。
- 運ぶのは `VtModes` 丸ごと。`Copy` + `PartialEq` で Bevy 型を含まないので D7 protocol
  purity を満たす。`mouse_encoding` は GUI では使わない（符号化は従来通り
  `OrzmaTty::send_mouse` がバックエンドで行う）が、ボタン PR では `mouse_tracking` の
  レベル差（Clicks / Drag / Motion）が要るので切り出さない。
- コンポーネントへの書き込みは `set_if_neq` を使う。リポジトリ既存の語彙であり
  （`crates/bevy_orzmux/src/title.rs:30`、`crates/bevy_orzmux/src/layout.rs:120`）、
  `.claude/rules/rust.md` の change detection ルールが求める形そのもの。
  `set_changed()` / `bypass_change_detection()` は使わない。
- **死んだ `VtSignal::ModeChange` と `TtyModeChangedSignal` は PR-1 で引退させる。**
  既に存在し drain にも配線済みだが、doc 自身が「`OrzmaVt` never raises it」と書いており
  （`crates/orzma_vt/src/lib.rs:305-314`、`crates/bevy_orzmux/src/signals.rs:35-43`）、
  構築している箇所はツリー内に無い。文字列型の死んだ mode シグナルを残したまま
  `TtyModesSignal` を足すと mode 系シグナルが 2 本並ぶ。置き換えれば PR-1 のシグナル増減は
  差し引きゼロになる。`bevy_orzmux` はワークスペース内部のパス依存なので外部影響は無い。
- モードが GUI に届くタイミングは、orzmux バックエンドが非同期に動き Bevy が update ごとに
  チャネル到着分を drain する以上、決まったフレーム数では保証されない。アプリが追跡を
  有効にした直後のノッチが旧経路に流れる窓は 0 〜 数フレームで、実害は無い。

## 5. ルータ契約 — `crates/orzma_tty/src/input/wheel.rs`

コメントアウトされている旧実装を置き換える。`TermMode`（alacritty）→ `VtModes` に
置き換え、戻り値を生バイトから意味レベルに変える。

```rust
pub enum WheelDecision {
    Report { button: MouseButton, count: u32 },   // マウス追跡中
    CursorKeys { key: TerminalKey, count: u32 },  // alternate scroll
    ScrollViewport(i32),                           // スクロールバック
    Noop,
}

impl WheelDecision {
    pub fn route(modes: VtModes, notches: i32, mods: WheelModifiers, cfg: &WheelConfig) -> Self;
    pub fn route_horizontal(modes: VtModes, notches: i32, mods: WheelModifiers, cfg: &WheelConfig) -> Self;
}
```

生バイトではなく意味レベルにすることで、呼び出し側は `mouse_encoding` を知らずに済み、
既存の `RequestTtyMouseInput` / `RequestTtyKeyInput` / `TerminalViewportScroll` に
そのまま落ちる。protocol に生バイト書き込み経路を足す必要も無い。旧実装が引数に
取っていた `mouse_cell` は不要になる（セルと `ProtocolModifiers` は呼び出し側が
`MouseReport` を組むときに付ける）。

### 型名の根拠

`WheelAction` / `WheelEffect` / `WheelReport` はいずれも採らない。`Action` は repo 内で 8 型・
3 つの別の意味に埋まっており（`ViModeAction` / `PaneAction` のバインド可能コマンド、
`PasteAction` / `CopyAction` の `EntityEvent`、`LocalButtonAction` の純粋ルータ決定）、`Effect`
も `KeyEffect` / `MouseEffect` が占めている。`Report` は `MouseReport` と DSR / DA の応答バイト
（`crates/orzma_vt/src/lib.rs:231`）という VT 語彙に二重に埋まっている上、4 バリアント中 1 つしか
レポートではない。`Verdict` は webview マウントの受理 / 拒否判断として `orzma_tty` 自身の中で
既に使われている（`crates/orzma_tty/src/lib.rs:259`、`test_support.rs:101`）。

`Decision` は型宣言としての衝突がゼロで、prose の 9 ヒットは全て `src/input/hyperlink.rs` の
`cursor_decision`（ホバー対象からカーソル形状を返す純粋な分類器）という**同じ意味**の使用なので、
語彙が割れるのではなく補強される。ホイールバインディングを将来入れると「バインディングが奪った」
4 つ目のバリアントが増えるが、その拡張にも耐える名前である。

コンストラクタは `route` / `route_horizontal` のまま。`.claude/rules/rust.md` の Constructors 節が
求める「`T::` 接頭辞が持つ型名を落として何をするかで命名する」を満たし、`route` は
`LocalButtonAction::route`（`src/input/mouse/button.rs:99`）として input 層に定着している。

バリアント名 `CursorKeys` は `ArrowKeys` ではなく VT の語彙を採る。`orzma_tty` 自身がこの分類を
「cursor keys」と呼んでおり（`crates/orzma_tty/src/input/keyboard.rs:68` "The cursor keys — the
arrows plus Home and End"、`cursor_key_bytes`）、DECCKM も DEC Cursor Key Mode である。

なお、この判断を型として持つ端末エミュレータは調べた範囲に存在しない。Alacritty
（`Processor::scroll_terminal`）、WezTerm（`TerminalState::mouse_wheel`）、kitty（`scroll_event`）、
foot（`mouse_scroll`）、Ghostty（`Surface.scrollCallback`）、Rio はいずれも 1 つの関数の中で
`if / else if / else` で分岐しており、借りられる既存の名前は無い。

### 縦の優先順位

1. `modes.mouse_reporting_active()` **かつ `!mods.shift`**
   → `Report { WheelUp | WheelDown, min(|notches|, cfg.max_protocol_events_per_frame) }`。
   `lines_per_notch` は掛けない。1 ノッチが何行かはアプリが決める。
2. `modes.alternate_scroll_active()`
   → `CursorKeys { ArrowUp | ArrowDown, min(|notches|, cfg.max_protocol_events_per_frame) × lines_per }`
3. それ以外 → `ScrollViewport(notches × lines_per)`（飽和乗算）

`lines_per` は `mods.fine` なら `cfg.fine_lines`、さもなくば `cfg.lines_per_notch`。
`notches == 0` と、上限で 0 に丸められた場合は `Noop`。

**上限は経路 1 だけでなく経路 2 にも、`lines_per` を掛ける前に適用する。** バースト上限は
PTY を入力の洪水から守るためにあり、`lines_per` 倍される矢印キー経路の方が本数は多い。
`cells_per_notch` の既定は 0.5（`src/input/bindings.rs:107`）で、`MouseScrollUnit::Pixel`
のデルタを行高で割るため、トラックパッドの 1 フリックが 1 フレームで数十ノッチを生みうる。
既定では経路 2 の上限は 8 × 3 = 24 本になる。上限を超えたノッチは**意図的に捨てる**
（アキュムレータに残して後続フレームで消化する慣性方式は採らない。指を離した後も数フレーム
スクロールが続き、遅延と受け取られる）。

`modes.mouse_reporting_active()` は `alternate_scroll_active()` の隣に足す新しい述語で、
`mouse_tracking != MouseTracking::Off` を意味する。`alternate_scroll_active()` の doc は
既に「an active mouse tracking mode outranks alternate scroll, and the caller must resolve
that order itself」と書いており、その相方が無い状態を埋める。優先順位が 2 つの bool 呼び出しで
表現でき、GUI は `MouseTracking` 自体を import せずに済む（ボタン PR ではレベル差が要るので
そちらでは import する）。

### 設定型と修飾キー型の所在

`WheelConfig` と `WheelModifiers` はルータと同じ `orzma_tty::input::wheel` が持つ。
`crates/orzma_tty/src/input.rs` に `pub use wheel::*;` を足すだけでよい —
`crates/orzma_tty/src/lib.rs:32` が既に `input::*` を prelude に再エクスポートしているので、
prelude への露出は自動で付いてくる（現状 `mod wheel;` は宣言済みだが中身が全てコメントアウトの
ため空になっている）。

`src/input/bindings.rs` のローカルな `WheelConfig` は、この型に置き換える。フィールドは
`lines_per_notch` / `fine_lines` / `max_protocol_events_per_frame` で完全に一致しており、
差し替え前は `OrzmaMouseConfig::wheel` がエンジンクレートの同名型そのものだった
（旧 `bindings.rs` は `use orzma_tty_engine::{ButtonConfig, WheelConfig};`）。
`ButtonConfig` はボタン PR までローカルのまま残す。

`WheelModifiers` の方は移設ではなく**新設**の型である（今はツリー内に存在せず、コメントアウト
された旧ルータの中にしか出てこない）。フィールドは `{ shift, fine }` の 2 つに絞る。旧実装の
`{ shift, ctrl, alt, fine }` のうち `ctrl` / `alt` は、符号化がルータの外に出た新設計では一度も
読まれず、全呼び出し側と全テストが埋めるだけの死フィールドになる。レポートに載る修飾キーの束は
呼び出し側が組む `ProtocolModifiers` の側にある。

`WheelDecision` は `Copy` を derive できない。`TerminalKey` が `Copy` ではないため
（`crates/orzma_tty/src/input/keyboard.rs:27`、`Character(KeyText)` が `String` を持つ）
`Clone` 止まりになる。矢印キーの clone はヒープ確保を伴わないので実害は無い。

ついでに、使用箇所ゼロの `pub enum WheelDir`（`crates/orzma_tty/src/input/mouse.rs:15`）を PR-2 で
削除する。ツリー全体で定義 1 箇所のみで参照が無く、新設計は方向を
`MouseButton::Wheel{Up,Down,Left,Right}` で表すため、PR-2 がこれを恒久的な死 public API にする
瞬間になる。

### 横

経路 1 のみ（`cb` 66 = Left / 67 = Right）。それ以外は `Noop`。横のスクロールバックも
alternate scroll も存在しない。

### 符号の約束

**旧実装から反転させる。** 旧ルータは「負 = 上」だったが、現行 `wheel.rs` は
「`raw_v` 正 = ホイール上 = 古い出力方向」で `Scroll::Delta` の正方向と一致しており、
無変換で渡している。新ルータもこちらに揃える:

- `notches > 0` = ホイール上 = 古い出力方向 → `WheelUp` / `ArrowUp` / `ScrollViewport` 正。

旧実装にあった `-raw_v` の反転とその NOTE は、この約束のもとでは復活させない
（現行 `wheel.rs` には既に存在しない）。

### 旧実装からの意図的な逸脱: 矢印キーは DECCKM を尊重する

旧ルータは alternate scroll の矢印を「DECCKM に関わらず常に SS3（`ESC O A`）」で
送っていた。新設計では既存の `RequestTtyKeyInput` に流すので
`PtyInput::encode_key`（`crates/orzma_tty/src/input.rs:37`、実際の分岐は
`crates/orzma_tty/src/input/keyboard.rs:85-88` の `cursor_key_bytes`）が DECCKM を尊重し、
app cursor が立っていれば `ESC O A`、さもなくば `ESC [ A` になる。

**これは逸脱ではなく、モードの生みの親である xterm への準拠である。** xterm の
`AlternateScroll()`（`scrollbar.c`）は
`reply.a_type = ((xw->keyboard.flags & MODE_DECCKM) ? ANSI_SS3 : ANSI_CSI);` と書いており、
alternate scroll の矢印で DECCKM を尊重している。無条件 `ESC O` を送る Alacritty の方が外れ値で、
旧実装はそちらに合わせていた。

補強として、nvim は terminfo の `keypad_xmit`（`smkx`、xterm 系では `\E[?1h\E=`）で DECCKM を
立て、`less` / `man` も同じく `smkx` を送るので、どちらの流儀でも実挙動は変わらない。見返りとして
protocol に生バイト書き込みコマンドを足さずに済む。

副次的に、旧 `alt_screen_arrow_bytes` にあった `unreachable!()` が構造的に消える。
`.claude/rules/rust.md` は非テストコードでの `unreachable!` を禁じているので、意味レベルの
戻り値はルール適合の面でも正しい形になる。

`OrzmaTty::send_key` はスクロールバックを live tail にスナップするが、この経路は
alt screen 上でしか通らず `display_offset` は常に 0 なので影響しない。

## 6. GUI ディスパッチ — `src/input/mouse/wheel.rs`

ノッチが 1 つ以上確定した後で初めてカーソル → セル射影と `TtyModes` 読みを行う。
旧実装の遅延射影を踏襲し、サブノッチのフレーム（ポインタ移動中の大半）で行列逆変換を
払わない。

| `WheelDecision` | trigger |
|---------------|---------|
| `Report { button, count }` | `RequestTtyMouseInput { terminal, mouse: MouseReport { button, kind: Press, cell, mods } }` を count 回 |
| `CursorKeys { key, count }` | `RequestTtyKeyInput { terminal, key, modifiers }` を count 回 |
| `ScrollViewport(lines)` | `TerminalViewportScroll { entity, lines }` |

旧実装は count 本を 1 つの `Vec<u8>` に連結して 1 回で書いていたが、新設計では
`RequestTtyMouseInput` / `RequestTtyKeyInput` を count 回 trigger するので、チャネル送信も
PTY への書き込みも count 回になる（矢印キー経路では `OrzmaTty::send_key` が 1 本ごとに
`snap_to_live_tail` + `write_all` を呼ぶ、`crates/orzma_tty/src/lib.rs:282-287`）。§5 の
上限により 1 フレームあたり経路 1 で最大 8 回、経路 2 で最大 24 回に収まるので許容する。
足りないと分かった場合の次の一手は、`OrzmuxCommand::MouseInput` / `KeyInput` に
`count: u32` を足してバックエンドで 1 回にまとめることで、コマンドを増やさずに旧実装の
コストに戻せる。今回は入れない。

- `TtyModes` は `TerminalSurfaces` クエリに足さず、**別の `Query<&TtyModes>` から可謬に
  引く**（`.get(target).copied().unwrap_or_default()`）。`src/session/spawn.rs:41-43` は
  サーフェスを `OrzmaTerminal` 単体で spawn し、`OrzmuxPane`（したがって `TtyModes`）は
  `PaneOpened` で後から付く（`crates/bevy_orzmux/src/drain.rs:71-75`）ため、`&TtyModes` を
  必須にするとペインが開くまでの窓でサーフェスがクエリから外れ、ホイールが死ぬ。この形なら
  §8 の「既存 8 テストは無改修」も本当に成り立つ（フィクスチャは `OrzmuxPane` を持たない）。
- 横は macOS のみ符号反転する。winit の macOS は物理右スワイプを負の `MouseWheel.x` で
  報告する（旧コードの NOTE を復活させる）。判定は `#[cfg(target_os = "macos")]` 属性では
  なく `cfg!(target_os = "macos")` 式で書く。両分岐が全ターゲットで型検査され、テストが
  Linux の CI から黙って消えない。
- Alt は fine modifier の既定であると同時にレポートの meta ビット（+8）でもある。旧実装
  通り `WheelModifiers { shift, ctrl, alt, fine }` → `ProtocolModifiers { meta: alt }`
  に写す。ただし旧コードの NOTE が書いていた「`alt` に入れるとビットが落ちる」は現在の
  実装では誤りで、`MouseReport::cb_bits` は `alt || meta` で +8 にしており
  （`crates/orzma_tty/src/input/mouse.rs:139`）、型の doc 自身が「merge into the single +8
  bit and never double-count」と書いている（同 `:25-27`）。どちらに入れてもバイト同値なので、
  この NOTE は復活させない。
- モジュール冒頭の `TODO: reintroduce app-forward wheel-reporting` を削除。
- `WheelConfig::max_protocol_events_per_frame` の `#[expect(dead_code)]` を削除。
  `OrzmaMouseConfig::buttons` の方はボタン PR まで残す。
- システム本体は ~150 行規則を守るためヘルパ `fn` に分割する。

### macOS: Shift+ホイールは縦として届かない

**決定 2（Shift = フォールスルー）は、正規化を入れないと macOS では到達不能になる。**
リポジトリ自身が `src/input/bindings.rs:6-8` に「On macOS, Shift+wheel becomes horizontal
scroll at the OS level, so Shift never reaches the app as vertical `y`」と記録している
（`FineModifier::Shift` が macOS で発火しない理由でもある）。何もしなければ macOS の
Shift+ホイールは横優勢ジェスチャとして `route_horizontal` に入り、追跡中なら `WheelLeft` /
`WheelRight`（cb 66 / 67）が nvim に飛ぶ。§10 の手動確認は再現しない。

対処: **`cfg!(target_os = "macos")` かつ Shift 保持のフレームでは、軸ロックに掛ける前に横
デルタを縦デルタへ加算する**（`delta_v += f(delta_h); delta_h = 0.0;`）。`mods.shift` は真のまま
保つので、ルータは経路 1 を飛ばして経路 2 / 3 に落ちる。代償として macOS では「Shift を押しながらの
意図的な横ジェスチャ」が表現できなくなるが、Shift+縦を横に変換しているのは OS 自身なので、横が
欲しいユーザーは Shift 無しで横スワイプすればよい。

**代入ではなく加算でなければならない。** macOS が Shift+スクロールを横に変換するのは
ディスクリートホイールのときだけで、トラックパッドでは Shift が無視されて縦のまま届く。代入に
すると、トラックパッドの Shift+2 本指スクロールが `delta_v := f(0.0) == 0` になり、決定 2 が
救おうとしているまさにその経路でスクロールが死ぬ。

**読み替えを有効にしたフレームでは `residual_cells_h` を明示的にクリアする。** `WheelAccumulator`
は軸ごとの残差を持ち、`retarget` 以外では消えない（`src/input/mouse/gesture.rs:99-153`）。デルタ
段階で軸を移すと横側は `accumulate_notches(.., 0.0, ..)` の no-op になり、Shift 前のフレームが
積んだサブノッチ端数が凍結したまま残って、次の本物の横ジェスチャで古い値から再開してしまう。
これは既存の「抑制された軸の残差を消すな」という NOTE とは矛盾しない — あちらは軸ロックが
デルタを**抑制**する話で、こちらは**移動**する話である。

`accumulate_wheel` は現在 `(gesture_acc, wheel, cell_h, cfg)` で修飾キーを知らない
（`src/input/mouse/wheel.rs:88-93`。`keys` が読まれるのは 1 つ先の `apply_vertical_scroll`）。
Shift を引数に足す。可変引数の後に置くこと（パラメータ順序規則）。

**`FineModifier::Shift` との関係を決めておく。** 正規化が入ると macOS でも Shift が縦として
届くようになるので、`src/input/bindings.rs:6-8` の doc（「Shift never reaches the app as
vertical `y`」）は macOS + ホイールで偽になり、PR-2 で更新が要る。同時に `FineModifier::Shift`
を設定しているユーザーでは Shift が「fine scroll」と「1 段下へフォールスルー」の両方を意味する
ことになる。**フォールスルーを優先し、fine は適用しない**（Shift を fine に割り当てている時点で
macOS では元々発火しなかったので、失うものは無い）。既定の fine modifier は Alt なので、
既定構成では衝突しない。

旧実装はこの衝突を別の向きで解いていた（`build_wheel_modifiers_horizontal` が macOS で Shift
ビットを落とし、横レポートを素の scroll-left/right に保つ）。旧実装には Shift フォールスルーが
無かったため、その解で足りていた。

### 横軸の符号は実機で決着させる

読み替えの符号と、横経路の `cfg!(target_os = "macos")` 反転そのものが、**証拠の食い違う争点**に
なっている。

- 旧 orzma コードが出荷した NOTE は「macOS/winit は物理右スワイプを負の `MouseWheel.x` で報告
  する（X11/Wayland と逆）」と主張する。実機で観測した経験的記述と思われる。
- winit 0.30.13 のソースはこれと逆を示す。macOS backend は `NSEvent.scrollingDeltaX` を符号反転
  せずそのまま `LineDelta` に渡し（`platform_impl/macos/view.rs:668-676`）、`MouseScrollDelta` の
  doc は「Positive values indicate that the content that is being scrolled should move **right**
  and down」と書く（`event.rs:951-958`）。X11 backend は button 6（wheel-left）→ `+1.0`、
  button 7（wheel-right）→ `-1.0` を割り当てる（`x11/event_processor.rs:1086-1089`）。つまり
  winit の意図としては両プラットフォームが「正 = content が右へ」に正規化されており、
  プラットフォームごとの反転は不要ということになる。
- さらに、符号は natural scrolling というユーザー設定にも依存し、winit は
  `NSEvent.isDirectionInvertedFromDevice` を公開していないので、`cfg!` というコンパイル時定数
  では原理的に表現できない可能性がある。

したがって **§10 の手動確認 5 を決着手段とする**。ディスクリートホイールとトラックパッドの両方で
確認すること（macOS では 2 つが別の経路を通るため）。観測結果に応じて、読み替えの符号と横経路の
反転の有無を同時に確定させ、macOS ゲートのテストを 1 本置く。符号ヘルパは横経路と共有するので、
**横の反転を残すなら読み替えも同じ向きに揃える必要がある** — 揃え損なうと Shift+ホイール上が
下にスクロールする。

## 7. orzma_vt 側の変更

### alternate scroll の既定 ON

`VtModes` の `#[derive(Default)]` は**残したまま**、`alternate_scroll: bool` を 2 値 enum に
置き換えて `#[default]` を立てる:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlternateScroll {
    #[default]
    Enabled,
    Disabled,
}
```

手書き `impl Default` は採らない。全フィールドを書き出す必要があり、`AutoWrap::Enabled` や
カーソル表示のように `false` / ゼロ値でない既定を持つフィールドを取りこぼす脆さがそのまま残る。
enum + `#[default]` は同じファイルの `AutoWrap`（`crates/orzma_vt/src/device/modes.rs:104-118`）、
`TextCursorEnable`、`InsertReplaceMode`、`ScreenKind` が全て採っている既存の書き方で、
`from_decset` の相方も揃う。呼び出し側は `alternate_scroll_active()` 越しにしか触らないので、
enum 化のコストはゼロ。

`alternate_scroll` が false であることを主張する既存テストは無いので、既定反転による破壊は
起きない。

- `interpreter.rs:778` の 1007 アームは `AlternateScroll::from_decset(enabled)` になり、
  隣の `7 => self.device.set_auto_wrap(AutoWrap::from_decset(enabled))` と同じ形に揃う。
- RIS / `Device::reset` は `self.modes = VtModes::default()`（`crates/orzma_vt/src/device.rs:165`）
  を通るので、既定 ON がそのまま復元される。
- 設定ファイルでの on/off は今回は入れない（YAGNI）。必要になったら `[mouse]` に足す。

### private mode の catch-all を明示アームにする（ついで作業）

`set_private_modes`（`crates/orzma_vt/src/interpreter.rs:795`）は今
`_ => self.set_mouse_mode(mode, enabled)` と書かれていて、明示的に処理しない private mode を
全てマウスモードハンドラに流している。**バグではない** — 明示アームが持つ番号
`{1, 3, 6, 7, 12, 25, 47, 66, 1004, 1007, 1034, 1047, 1048, 1049, 2004}` と `set_mouse_mode`
が答える `{1000, 1002, 1003, 1006}` は交差せず、`match` は上から評価されるので catch-all が
横取りすることもなく、どちらの `with_decset` も知らない番号は黙って捨てられるので結果は
`_ => {}` と同じになる。それでも形として直す価値がある:

- interpreter.rs の他 4 箇所の catch-all（`:150`、`:234`、`:287`、`:486`）は全て
  「明示アーム + `_ => {}`」で、ここだけ「落ちてきたものを処理する」形に反転している。
- 実装済みの 1000 / 1002 / 1003 / 1006 が `match` を読んでも一切現れず、未実装に見える。
  コメントは TODO / NOTE / SAFETY に限られるので、説明で補うこともできない。
- 「未対応の private mode」と「マウスモード」が同じ枝なので、将来 orzma_vt に観測手段を
  入れたくなったときに未対応 DECSET を切り分けられない（現状 orzma_vt は `tracing` を
  持たず依存を最小に保っているので、今すぐの作業ではない）。

PR-2 で次の形に直す:

```rust
// TODO: Route 1005 to an encoding once `MouseEncoding::with_decset` answers it.
1000 | 1002 | 1003 | 1005 | 1006 => self.set_mouse_mode(mode, enabled),
_ => {}
```

番号リストが `set_private_modes` と `with_decset` の 2 箇所に散るのが代償だが、乖離は常に
安全側に倒れる。アームにあって `with_decset` が答えない番号（1005 がまさにそれ）は無視される
だけで、むしろ「ここに届いて今は捨てられる」ことが可視化される。逆向きの乖離は dead code に
なるが、番号を足した本人がその場で気づく。

挙動は変わらないので、新しいテストは足さない。既存の 6 本がそのまま回帰検出になる —
`crates/orzma_vt/src/interpreter/tests/mouse.rs` の 4 本、
`interpreter::tests::soft_reset::a_soft_reset_leaves_the_mouse_and_paste_modes_alone`、そして
最も直接的な `interpreter::tests::private_modes::an_unknown_private_mode_is_ignored`
（`\x1b[?9999h` を流して catch-all の no-op を固定している、
`crates/orzma_vt/src/interpreter/tests/private_modes.rs:34`）。

## 8. テスト

| 層 | 内容 |
|----|------|
| `orzma_tty`（ルータ・純粋関数） | 優先順位 3 経路、Shift フォールスルー（追跡中 + Shift → alt screen なら矢印キー / 通常画面ならスクロール）、経路 1 / 経路 2 それぞれのバースト上限（経路 2 は `lines_per` を掛ける前に適用される）、fine modifier、0 ノッチ、追跡外の横ホイールは `Noop` |
| `orzma_vt` | 1007 の既定が `AlternateScroll::Enabled` / DECRST 1007 で `Disabled` / `alternate_scroll_active()` は alt screen のときだけ true / RIS で既定に戻る / `mouse_reporting_active()` は `MouseTracking::Off` のときだけ false。catch-all の明示アーム化は挙動不変なので、既存の 6 本（`interpreter/tests/mouse.rs` の 4 本 + `soft_reset` の 1 本 + `private_modes::an_unknown_private_mode_is_ignored`）を回帰検出として使い、新規テストは足さない |
| `orzmux` | `\x1b[?1002h`（nvim が実際に送る追跡モード）を流すと `OrzmuxEvent::Modes` が 1 度出る、無変化では出ない、同一チャンク内で複数回遷移しても最終状態の 1 イベントに合流する（`pump_pane` 呼び出し単位では最大 `PUMP_ROUNDS` 回出うるので、そこは主張しない）。既存の `enable_focus_reporting` ハーネス（`crates/orzmux/src/backend.rs:1006`）が雛形 |
| `bevy_orzmux` | `TtyModesSignal` → `TtyModes` 反映、同値では書かない |
| `src/input/mouse/wheel.rs` | 既存 8 本のうち**ディスパッチ層の 6 本は無改修で通る**。フィクスチャは `OrzmuxPane` を持たず `TtyModes` も無いが、§6 の可謬取得が `VtModes::default()`（追跡 off）に落ちるため経路 3 に入る。残る 2 本（`scroll_lines_uses_lines_per_notch_by_default`、`scroll_lines_fine_modifier_uses_fine_lines`、`src/input/mouse/wheel.rs:359-375`）はローカルの `fn scroll_lines` を直接呼んでおり、§5 がそれをルータの経路 3 に吸収するので `orzma_tty` 側へ移す。追加: 追跡中 → `RequestTtyMouseInput` × n、alt screen のみ → `RequestTtyKeyInput`、Shift + 追跡 + alt screen → 矢印キー、macOS ゲートで Shift+横デルタが縦に加算される |

テストの doc comment は `.claude/rules/rust.md` の「first line + `Case:` のみ」に従う。

## 9. PR 分割

- **PR-1 モード伝播**: `OrzmuxEvent::Modes` + backend 差分 + `TtyModesSignal` +
  `TtyModes` コンポーネント。単体では挙動を変えない。
- **PR-2 ホイールルーティング**: ルータ復活 + `wheel.rs` 振り分け + §7（`AlternateScroll` enum
  による 1007 既定 ON、`VtModes::mouse_reporting_active()` の追加、private mode catch-all の
  明示アーム化）+ 死んだ `WheelDir` の削除 + `src/input/bindings.rs:6-8` の doc 更新。
  **nvim でスクロールできるようになるのはここ**。
- **PR-3（今回のスコープ外）ボタン / ドラッグ転送**: `TtyModes` をそのまま使う。
  `src/input/mouse/button.rs` の TODO と `OrzmaMouseConfig::buttons` の
  `#[expect(dead_code)]` を解消する。

## 10. 手動確認

PR-2 の後、`cargo run` して:

1. `nvim <長いファイル>` でホイールが効く。
2. `less <長いファイル>` でホイールが効く（alternate scroll、追跡なし）。
3. シェルに戻ってホイールを回すとスクロールバックが動く（回帰していない）。
4. `nvim` 内で横スワイプしても縦に漏れない。
5. **横軸の符号の決着**（§6「横軸の符号は実機で決着させる」）。macOS で、**ディスクリート
   ホイールとトラックパッドの両方**で試す:
   - `nvim` 表示中に Shift+ホイールを回し、矢印キー経由で意図した向きにスクロールするか。
   - 追跡中のアプリの上で素の横スワイプを行い、物理右が `cb 67`（Right）として届くか。

   観測結果に応じて、読み替えの符号と横経路の `cfg!` 反転の有無を同時に確定させ、macOS ゲートの
   テストを 1 本置く。両者は符号ヘルパを共有するので、必ず同じ向きに揃えること。
