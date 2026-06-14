# ratatui-ozma フォーカス/入力API 再設計（H: host-tunnel）

- 日付: 2026-06-15
- ステータス: Approved（discussion-board での分析 + オーナー裁定）
- 関連: [2026-06-14-ratatui-ozma-webview-focus-design.md](2026-06-14-ratatui-ozma-webview-focus-design.md)（初版。本書が置換）
- クロスリポジトリ: ozmux（本リポ）+ bevy_cef ローカル worktree `$HOME/workspace/bevy_cef/wt/passthrough`（branch `passthrough`）

## Context / 背景

初版の `FocusManager` 系「フォーカス管理フレームワーク」を廃止し、フォーカスをアプリ責務へ戻す再設計。目標:
1. `FocusManager` 等フォーカス制御を強制する API を廃止。
2. `WebviewWidget::focused(bool)` を残し、ウィジェットが Focus 状態なら webview ページのフォーカスも自動同期（描画でフォーカス表現 → SDK が同期）。
3. `NavKeymap`（ナビ専用）を「webview からパススルーする任意ショートカット一覧」へ一般化。
4. **webview フォーカス中でも、宣言したパススルーキーが押されたら `crossterm::event::read()` で `Event::Key` として読める**。

discussion-board（6名）は SDK 単純性/ratatui 純粋性の観点から **S（SDK event-merge）** を 4:2 で支持したが、オーナーは「アプリが普通に `event::read()` を書ける」エルゴノミクスを優先し **H（host-tunnel）** を選択。ホスト + bevy_cef のコストは、**bevy_cef のローカル worktree を用意して改修する**前提で受容。S 案は付録（不採用案）として末尾に保持。

## 確定アーキテクチャ（H）

### 全体フロー
- **webview 非フォーカス時**: ホストが全キーを PTY へ流す（従来どおり）→ アプリの `event::read()`。
- **webview フォーカス時**: ホストはキーを CEF へ流し PTY を抑止（従来）。**ただし宣言パススルーキーだけは例外**: ホストが PTY へ転送（→ `event::read()`）し、bevy_cef はそのキーを CEF へ送らない。
- glue はパススルーのキー横取りを**しない**（ホストが処理）。`window.ozmux.call/on` ブリッジは既存 RPC 用途で存続。

### A. bevy_cef 改修（ローカル worktree `$HOME/workspace/bevy_cef/wt/passthrough`）
二重配送（host が PTY へ流しても bevy_cef が独立に CEF へ送る）を止める唯一の手段。`src/keyboard.rs` の `send_key_event`（line 104-140）に embedder 用の抑止フックを追加:

```rust
/// Embedder が「フォーカス webview に送らないキー」を毎フレーム宣言するためのリソース。
#[derive(Resource, Default)]
pub struct SuppressedKeys { /* HashSet<(Entity, KeyCode, KeyModifiers)> 等 */ }

impl SuppressedKeys {
    pub fn contains(&self, webview: Entity, code: KeyCode, mods: KeyModifiers) -> bool { /* ... */ }
    pub fn set(&mut self, ...) { /* embedder が書く */ }
}

fn send_key_event(/* ... */, suppressed: Res<SuppressedKeys>) {
    // ...
    let Some(webview) = target else { continue; };
    if suppressed.contains(webview, event.key_code, modifiers) {   // ★追加
        continue;   // ozmux が PTY へ流すので CEF へは送らない
    }
    for key_event in create_cef_key_events(modifiers, event) {
        browsers.send_key(&webview, key_event);
    }
}
```
- `KeyboardPlugin::build` で抑止リソースを `init_resource` + `pub` export。Windows 側 `send_key_event_win` にも同じチェック（構造同型）。
- **命名は embedder 非依存に**（`SuppressedKeys` 固有名でなく `CefKeyboardInputFilter`/`KeyboardInputPolicy` 等の汎用リソース。中身は「この Entity へ送らないキー集合」または述語）。upstream（not-elm/bevy_cef）へ filter フックとして取り込みやすくする（当面 worktree、最終的に PR 化）。
- **public SystemSet を export**（§C の順序付けで ozmux が `.before(set)` を指定できるように。現状 `send_key_event` は private system で外部から順序指定不可）。

### B. ozmux ↔ bevy_cef の依存差し替え（Cargo）
ozmux は bevy_cef を git 依存（`not-elm/bevy_cef` branch `headless-gpu`, rev e6c7b44）で参照（root `Cargo.toml:25,83`）。開発中はワークスペース root `Cargo.toml` に `[patch]` を追加してローカル worktree へ差し替え:
```toml
[patch."https://github.com/not-elm/bevy_cef"]
bevy_cef = { path = "/Users/taiga/workspace/bevy_cef/wt/passthrough" }
bevy_cef_core = { path = "/Users/taiga/workspace/bevy_cef/wt/passthrough/crates/bevy_cef_core" }
```
**注意**: (1) cited rev は古い場合あり（worktree HEAD は `8c3c8d7` 等、`e6c7b44` から進む）。(2) `[patch]` は**同名・同 version**でのみ成立 — `passthrough` worktree の `bevy_cef_core` は `0.11.0-dev`。`headless-gpu` が公開する version と一致しないと patch が黙って無効化されるので `cargo metadata` で実解決を確認。(3) `/Users/...` の path patch は user 固有で **CI/別環境では非再現** → CI では committed branch/rev を指す（path patch はローカル開発専用、コミットしない）。

### C. ホスト入力層の改修（`src/input.rs`）
1. **抑止リソースの充填**: フォーカス中 inline webview の `passthrough_keys`（register静的→`DynamicView`。できれば mounted `InlineWebview` entity にも resolved copy を持たせ focused child から即参照）を bevy_cef の抑止リソースへ書く。**順序付け**: bevy_cef `send_key_event` は private system(Update)で ozmux から `.before()` を直接指定できない → (a) ozmux 充填を `PreUpdate` に置く、または (b) bevy_cef が export した public SystemSet に `.before(set)` する(§A)。後者を推奨。
2. **`dispatch_focused_key` のゲート例外**（現状 `input.rs:244` 付近 `if focused_inline.is_some() { continue; }`）:
   ```rust
   if let Some(child) = focused_inline {
       if !passthrough_set(child).matches(&ev) {
           continue;                       // 宣言外: CEF のみ（PTY 抑止, 従来）
       }
       // 宣言キー: PTY へ転送（下の forward へフォールスルー）
   }
   // forward_to_active_terminal(...) で PTY へ（input.rs:249 付近）
   ```
   `passthrough_set(child)` は当該 webview の `DynamicView.passthrough_keys`。
   **★重要(precedence)**: `dispatch_focused_key` は inline ゲート(`input.rs:244`)の**前**に、グローバルショートカット lookup(`bindings.lookup`→`execute_action`→`continue`, `input.rs:223-242`)を実行する。passthrough 判定を `244` に置くと、ozmux ショートカットと衝突するチョードはゲート到達前に食われる。よって **passthrough 一致判定・PTY 転送は `223` のショートカット lookup より前に hoist** する（フォーカス webview の宣言キーを優先＝アプリが奪うことを明示決定。コピーモード起動等のグローバル shortcut を webview が遮蔽しうる点はトレードオフとしてドキュメント化）。

### D. エンジンの Alt/Meta 符号化修正（`crates/ozma_tty_engine` + `src/input.rs`）
Alt 修飾チョード（例 `Alt+h`）を PTY 経由で `event::read()` に `Alt+h` として届けるため、保留していた macOS Alt 入力対応を本対応に含める:
1. `src/input.rs` の `bevy_to_terminal_key`（input.rs:482）: Alt 押下時、合成され得る `logical_key` ではなく**物理 `key_code` から基底 ASCII 文字を復元**（KeyA..KeyZ/Digit）。US 配列前提（ドキュメント化）。
2. `crates/ozma_tty_engine/src/input_codec.rs` の `encode_key`: `mods.alt`/`mods.meta` 時に **ESC プレフィックス**を付与（xterm "meta sends escape"）。`ESC` + キーバイト。
- これにより crossterm が `Alt+h` を `ESC h` から復号。`Ctrl+key`/`Tab`/`F-keys` は既存符号化で動作（`Ctrl+h=0x08` 等の曖昧性はドキュメント注記）。

### E. パススルー宣言 = Webview::register 静的（control-plane/wire 拡張）
- SDK: `Webview::passthrough(keys: impl IntoIterator<Item = KeyChord>)` ビルダ（register 前に固定）。
- 型: `pub struct KeyChord { pub mods: KeyModifiers, pub code: KeyCode }`（crossterm 直結）。
- wire: `ClientMsg::Register`/`RegisterKind`（SDK `protocol.rs` と `src/control_plane/protocol.rs`）に `passthrough_keys` を追加 → `DynamicView` に保持（`src/control_plane.rs`）。
- host 入力層が `DynamicView.passthrough_keys` を参照（上記 C）。**glue へは送らない**（H ではキー処理はホスト側）。
- **★型正規化**: 照合キーは3空間に跨る — SDK 宣言=crossterm `KeyCode`/`KeyModifiers`、CEF 抑止=Bevy `KeyboardInput.key_code`(物理)、PTY 復元=`TerminalKey`。register 時に passthrough chord を host 内部表現(Bevy physical key + 正規化 modifier)へ**一度だけ正規化**して `DynamicView` に保持し、毎キーの変換とズレ(特に Alt 合成)を避ける。

### F. フォーカス同期 = widget 駆動・変化時のみ送信
- `WebviewWidget::focused(bool)`: `render` で `FramePlacements` に `(handle, focused)` 記録（描画は枠/タイトル強調のみ、純粋）。
- `Ozma::flush`: **アプリ宣言フォーカスの前フレーム差分が変化した時のみ** control-plane focus/blur op を送る（既存 `flush_placements` の差分機構/`FlushState` を拡張）。host 側 `apply_control_events` → `FocusedWebview`。
- **★FocusedWebview 三重 writer 問題**: 「変化時のみ送信」は SDK の**再送**を防ぐが、それだけでは不十分。host 側の `FocusedWebview` writer は他に2つ — (1) クリック(`mouse_buttons.rs:776-779`)、(2) **`sync_focused_webview`(`webview_render.rs:91-112`)が毎フレーム active pane から再導出**。アプリ宣言フォーカス op は第三の writer で、宣言先が active-pane 由来 target と異なると **`sync_focused_webview` が翌フレームに上書き(clobber)**する(line 109-111)。→ **対応**: アプリ宣言フォーカスを `sync_focused_webview` の権威とガートする: (a) 宣言フォーカスを「active surface の inline child を preserve」する既存アームに通す、または (b) 宣言時にマーカーを立て `sync_focused_webview` がそれを尊重(上書きしない)。flush 差分(再送防止)＋この権威ガートの**両方**が必要。複数 focused(true) は flush で決定論的単一化 + `debug_assert!`。
- クリックでアプリのフォーカス状態を更新する双方向同期は**初版スコープ外**（アプリは widget でフォーカスを駆動。クリック→アプリ反映は将来 OzmaEvent 的通知を足す余地）。

### G. SDK 単純化（API 一掃）
H では SDK はキー配送に関与しないため大幅に薄くなる。アプリは**全キーを `crossterm::event::read()` で読む**（パススルーも本物の crossterm イベント）。

**削除**: `FocusManager`, `FocusSync`, `Signal`, `focusable`, `NavKeymap`, `NavKey`, `Modifier`, `Direction`, `WebviewHandle::{set_nav_keys, set_page_focus, focus, blur, focus_instance}`, `keymap.rs`（`resolve_spatial` 含む）。S 案で検討した `OzmaEvent`/`poll_event`/イベント channel は**作らない**。

**残す/追加**: `WebviewWidget::{new, fallback, focused, is_focused}`（focused が同期駆動の唯一入口）、`Webview::{inline, dir, interactive, on, passthrough}`、`WebviewHandle::{id, emit}`、`KeyChord`、`Ozma::{connect, register, frame, flush}`。空間ナビ・フォーカスリングはアプリ自実装。

**注意（wire op は残す）**: `WebviewHandle::{focus, blur}`(public メソッド)は削除するが、**control-plane の `ClientMsg::Focus` wire op と host 側 `apply_control_events` の Focus 処理は残す** — flush 駆動フォーカス(§F)が wire op を使ってホストの `FocusedWebview` を設定するため。削除は public SDK サーフェスのみで、wire/host 経路は維持。

## 影響範囲（変更ファイル）

**bevy_cef worktree（`$HOME/workspace/bevy_cef/wt/passthrough`）**
- `src/keyboard.rs`: `SuppressedKeys` リソース + `send_key_event`/`send_key_event_win` の抑止チェック、`KeyboardPlugin` で init/export。

**ozmux — Cargo**
- root `Cargo.toml`: `[patch]` で bevy_cef/bevy_cef_core をローカルパスへ。

**ozmux — SDK (`sdk/ratatui-ozma/src/`)**
- `focus.rs`/`keymap.rs` 削除。`webview.rs`（`passthrough` builder 追加、focus/blur/set_nav_keys/set_page_focus 削除）。`widget.rs`（focused を FramePlacements 記録へ）。`session.rs`（flush の focus 差分送信）。`protocol.rs`（Register に passthrough_keys）。`lib.rs`（export 整理: KeyChord 追加、FocusManager 系削除）。

**ozmux — ホスト (`src/`, `crates/`)**
- `control_plane/protocol.rs`・`control_plane.rs`（RegisterKind/DynamicView に passthrough_keys）。`input.rs`（`dispatch_focused_key` ゲート例外 + `SuppressedKeys` 充填 + `bevy_to_terminal_key` Alt 基底文字復元）。`crates/ozma_tty_engine/src/input_codec.rs`（`encode_key` Alt/Meta→ESC）。bevy_cef へ `SuppressedKeys` を渡す配線（plugin 登録/システム順序）。

**ozmux — 例**
- `focus_grid.rs`/`ratatui_webview.rs` を新 API（`Webview::passthrough` + `WebviewWidget::focused` + 素の `event::read()` ループ）へ書換。アプリ側に簡単なフォーカスリングを自実装するデモ。

## データフロー（パススルー）
```
[webview フォーカス中に Alt+h]
  winit/Bevy KeyboardInput
  ├─ ozmux dispatch_focused_key: focused_inline + Alt+h ∈ passthrough
  │    → bevy_to_terminal_key(key_code→'h') + mods.alt
  │    → encode_key: ESC 'h' を PTY へ → アプリ event::read() が Alt+h を復号 ✓
  └─ bevy_cef send_key_event: SuppressedKeys.contains(webview, KeyH, ALT) → CEF へ送らない ✓
  (宣言外キーは従来どおり: ozmux は PTY 抑止 / bevy_cef が CEF へ)
```

## テスト計画
- bevy_cef: `SuppressedKeys.contains` 単体、`send_key_event` が suppressed キーを `browsers.send_key` しない（モック/既存テスト流儀）。
- エンジン: `encode_key` の Alt→ESC（`Alt+h`→`\x1b h`）、`bevy_to_terminal_key` の Alt 時 key_code 基底文字復元。
- ホスト: register に passthrough_keys が載り `DynamicView` に保持される経路、`dispatch_focused_key` のゲート例外（passthrough は forward、非passthrough は suppress）、`SuppressedKeys` 充填。
- SDK: `KeyChord` の register wire シリアライズ、flush の focus 差分（宣言変化時のみ op）、widget::focused 記録。
- 例: `cargo build --examples`。
- 実機: ペイン内で example 実行 → webview フォーカス中に宣言チョードが `event::read()` に届く、ページ内 textarea には入らない（二重配送抑止）、宣言外キーはページへ。

## スコープ外 / リスク
- **bevy_cef 改修は upstream 化が望ましい**（当面 worktree + `[patch]`。最終的に `headless-gpu` へ filter フックを取り込む）。
- `Ctrl+letter` の制御バイト曖昧性（`Ctrl+h=Backspace` 等）はドキュメント注記。
- クリック→アプリ・フォーカス双方向同期は初版スコープ外。
- macOS Alt 基底文字復元は US 配列前提（非US配列は将来）。

## 付録: 不採用案 S（SDK event-merge）
discussion-board が 4:2 で支持した案。SDK が `Receiver<OzmaEvent>` を提供しアプリが crossterm と merge（tui-realm Port/Poll 型）、host/bevy_cef 無改造。ratatui 純粋性・sans-io 整合・ホスト無改造が利点だが、アプリが merge ループを書く必要があり「素の `event::read()`」にならない。オーナーがエルゴノミクスを優先し H を選択。将来 H が困難化した場合の fallback として記録。
