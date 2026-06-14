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
- `KeyboardPlugin::build` で `init_resource::<SuppressedKeys>()`、`SuppressedKeys` を `pub` export。
- Windows 側 `send_key_event_win` にも同じチェック。
- 将来は upstream（not-elm/bevy_cef）へ filter フックとして取り込む想定（当面は worktree で開発）。

### B. ozmux ↔ bevy_cef の依存差し替え（Cargo）
ozmux は bevy_cef を git 依存（`not-elm/bevy_cef` branch `headless-gpu`, rev e6c7b44）で参照（root `Cargo.toml:25,83`）。開発中はワークスペース root `Cargo.toml` に `[patch]` を追加してローカル worktree へ差し替え:
```toml
[patch."https://github.com/not-elm/bevy_cef"]
bevy_cef = { path = "/Users/taiga/workspace/bevy_cef/wt/passthrough" }
bevy_cef_core = { path = "/Users/taiga/workspace/bevy_cef/wt/passthrough/crates/bevy_cef_core" }
```
（実パス・crate レイアウトは worktree を確認して調整。`passthrough` ブランチは `headless-gpu` 互換である前提。）

### C. ホスト入力層の改修（`src/input.rs`）
1. **`SuppressedKeys` の充填**: フォーカス中 inline webview の `passthrough_keys`（register静的→`DynamicView`）を毎フレーム/フォーカス変化時に bevy_cef の `SuppressedKeys` へ書く。bevy_cef の `send_key_event` より前に走る順序付け（PreUpdate or `.before(...)`）。
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

### F. フォーカス同期 = widget 駆動・変化時のみ送信
- `WebviewWidget::focused(bool)`: `render` で `FramePlacements` に `(handle, focused)` 記録（描画は枠/タイトル強調のみ、純粋）。
- `Ozma::flush`: **アプリ宣言フォーカスの前フレーム差分が変化した時のみ** control-plane focus/blur op を送る（既存 `flush_placements` の差分機構/`FlushState` を拡張）。host 側 `apply_control_events` → `FocusedWebview`。
- 「変化時のみ送信」によりクリック to フォーカス（host 側で `FocusedWebview` 直接 set）と競合しない: アプリがフォーカスを変えない限り flush は op を送らないので、クリックで付いたフォーカスを毎フレーム blur で奪わない。複数 focused(true) は flush で決定論的単一化 + `debug_assert!`。
- クリックでアプリのフォーカス状態を更新する双方向同期は**初版スコープ外**（アプリは widget でフォーカスを駆動。クリック→アプリ反映は将来 OzmaEvent 的通知を足す余地）。

### G. SDK 単純化（API 一掃）
H では SDK はキー配送に関与しないため大幅に薄くなる。アプリは**全キーを `crossterm::event::read()` で読む**（パススルーも本物の crossterm イベント）。

**削除**: `FocusManager`, `FocusSync`, `Signal`, `focusable`, `NavKeymap`, `NavKey`, `Modifier`, `Direction`, `WebviewHandle::{set_nav_keys, set_page_focus, focus, blur, focus_instance}`, `keymap.rs`（`resolve_spatial` 含む）。S 案で検討した `OzmaEvent`/`poll_event`/イベント channel は**作らない**。

**残す/追加**: `WebviewWidget::{new, fallback, focused, is_focused}`（focused が同期駆動の唯一入口）、`Webview::{inline, dir, interactive, on, passthrough}`、`WebviewHandle::{id, emit}`、`KeyChord`、`Ozma::{connect, register, frame, flush}`。空間ナビ・フォーカスリングはアプリ自実装。

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
