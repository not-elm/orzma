# ratatui-ozma フォーカス/入力API 再設計

- 日付: 2026-06-15
- ステータス: Approved（discussion-board 6名 / 4:2 ratified + オーナー裁定）
- 手法: discussion-board（ratatui-arch, sdk-api, host-cef ×(normal+cx) の構造化討議）
- 関連: [2026-06-14-ratatui-ozma-webview-focus-design.md](2026-06-14-ratatui-ozma-webview-focus-design.md)（初版フォーカス設計。本書はそれを置換する再設計）

## Context / 背景

初版で `FocusManager`（アプリ所有フォーカスリング: navigate/drain/FocusSync/空間ナビ/glue連携）+ `NavKeymap` + `WebviewHandle::{focus,blur,set_nav_keys,set_page_focus}` + `focusable()` という「フォーカス管理フレームワーク」を SDK が提供した。

オーナーの問題意識: **フォーカス管理は一般にアプリ側の責務であり、`FocusManager` のようにフォーカス制御を強制する API は SDK として不適切**。SDK は「webview を ratatui ウィジェットとして埋め込む」ための薄いプリミティブに徹し、フォーカスの所有はアプリに委ねるべき。

達成したいこと:
1. フォーカス制御を強制する `FocusManager` 系 API を廃止し、フォーカスはアプリが所有。
2. `WebviewWidget::focused(bool)` を残し、**ウィジェットが Focus 状態なら webview ページのフォーカスも自動同期**（アプリが描画でフォーカスを表現 → SDK が同期）。
3. `NavKeymap`（ナビ専用）を、より普遍的な「**webview からキー入力をパススルーする任意のショートカット一覧**」API に一般化。
4. **webview にフォーカスがある状態でも、宣言したパススルーキーが押されたら `Event::Key` としてアプリのイベントループに届く**。

## 設計判断の根拠（チーム討議）

6名（ratatui-arch / sdk-api / host-cef ×(normal + codex-explore)）で配送方式 S/H 他を討議。**4:2 で S（SDK event-merge）を ratified**。要点:
- **配送 S が ratatui 哲学（アプリがループ/IO 所有、ライブラリは入力非介入）と sans-io に忠実**。H（ホストが PTY 転送 + bevy_cef 二重配送抑止）は bevy_cef（path/git 依存, rev e6c7b44）のフォーク/パッチ（`send_key_event` に embedder filter 新設 + ozmux→bevy_cef keyset 受渡 + dispatch 同期）が必須で、IME/repeat/shortcut/inline-focus 保持と相互作用が大きい。
- 配送 S の唯一の論争点「glue の preventDefault がページ内 textarea へのキー挿入を止めないのでは」は**誤前提と実証**: 実 glue（`ozmux_bridge.js`）は **capture フェーズ + preventDefault + stopPropagation** を使用（preload で全ページより先に登録）。keydown の preventDefault はデフォルトのテキスト挿入を抑止し、capture+stopPropagation でページハンドラより先に遮断する。H 推奨だった host-cef-cx もコード確認後 S へ転向。
- **二重配送(PTY)が起きない真の理由**は preventDefault ではなく、ozmux 既存ゲート `focused_inline.is_some() → continue`（input.rs:244）で inline-focus 中は PTY 転送しないため。CEF だけがキーを受け、glue が宣言チョードを RPC で返す。
- **SDK はイベントループを所有しない**。`poll_event` で crossterm を「置換」するのは ratatui 原則違反。代わりに **`Receiver`（channel）を提供しアプリが merge** する（tui-realm Port/Poll 前例。既存 reader thread → crossbeam_channel → drain が既にこの形）。

## 確定アーキテクチャ

### A. パススルーキー配送 = S（SDK event-merge）

- ホスト/bevy_cef は無改造（後述 D の register 拡張を除く）。
- ページ glue（`webview_render/ozmux_bridge.js`）は、宣言された**パススルーチョード**を keydown(capture) で検知したら `preventDefault()` + `stopPropagation()` し、既存 RPC（`window.ozmux.call`）でアプリへ転送（現 `__ozma.nav` を一般化した `__ozma.key`）。
- **SDK は `Receiver<OzmaEvent>` を提供**。`OzmaEvent` は薄い enum:
  ```rust
  pub enum OzmaEvent {
      Key(ratatui::crossterm::event::KeyEvent),  // パススルーキー(再構築)
      FocusGained(WebviewHandle),                 // ページがDOMフォーカス取得(クリック等)
      FocusLost(WebviewHandle),                   // ページがDOMフォーカス喪失
  }
  ```
  （crossterm の `Event` 全面再エクスポートはしない。`KeyEvent` のみ再エクスポート。）
- アプリは自分の crossterm ループで両ソースを merge:
  ```rust
  loop {
      // 1. SDK channel を drain（webview フォーカス中のパススルー/フォーカス通知）
      while let Some(ev) = ozma.try_event() { match ev { OzmaEvent::Key(k) => /* 通常キー扱い */, ... } }
      // 2. crossterm（webview 非フォーカス時の通常入力）
      if event::poll(timeout)? { match event::read()? { Event::Key(k) => ..., } }
      // 3. draw + flush
  }
  ```
  SDK は `Ozma::try_event() -> Option<OzmaEvent>` / `Ozma::events() -> impl Iterator`（非ブロッキング drain）を提供。利便のため `Ozma::poll_event(timeout)`（crossterm と channel を merge する薄いヘルパ）も任意で提供してよいが、**プリミティブは channel**（アプリが merge を所有）。
- 内部: 既存 reader thread（`session.rs` spawn_reader）が control socket の `__ozma.key`/`__ozma.focus` を受けて `crossbeam_channel` の `Sender<OzmaEvent>` へ enqueue。`Ozma` が `Receiver<OzmaEvent>` を保持。

### B. フォーカス同期 = widget 駆動・単一 writer

- `WebviewWidget::focused(bool)`: `render` で `FramePlacements` に `(handle, focused)` を記録（描画自体は枠/タイトル強調のみ、純粋）。
- `Ozma::flush`: 前フレームとの**差分**で control-plane の focus/blur op を送信（既存 `flush_placements` の placement 差分と同型 / 同一 `FlushState` 拡張）。host 側 `apply_control_events` → `FocusedWebview` 設定（既存 op を再利用）。
- **単一 FocusedWebview writer**: 並列フォーカスチャネルを追加しない。host 側 `apply_webview_focus` の `is_changed` が重複排除。
- **複数 widget が focused(true)**: アプリが単一を保証する前提。SDK は flush で決定論的に単一化（最後に記録された1つを focus、他を blur）+ `debug_assert!` で誤用検出。
- **クリック to フォーカス調停**: クリックは host 側で `FocusedWebview` を直接 set（`mouse_buttons.rs`, bevy_cef `focus.rs`）。glue が DOM の focus/blur を `OzmaEvent::FocusGained/Lost` としてアプリへ通知 → アプリが自分のフォーカス状態を同期。SDK は blur op を「直近のクリックで focus されたばかりの webview」に対してガードし、blur が click に勝つ race を防ぐ。

### C. パススルーショートカットの型 = crossterm 直結

```rust
pub struct KeyChord {
    pub mods: ratatui::crossterm::event::KeyModifiers,
    pub code: ratatui::crossterm::event::KeyCode,
}
```
- 独自 `Modifier`/`NavKey`/`Direction` enum は廃止（現 `keymap.rs` は内部で crossterm へ変換しており二重定義）。
- アプリは `OzmaEvent::Key(KeyEvent)` を crossterm 型で受けるので、照合も crossterm 型で完結。
- glue への serialize は `KeyChord -> { mods: [...], key: "..." }`（現 `NavKeymap` の js_name ロジックを KeyChord 単位へ移植）。
- **制約（必須ドキュメント化）**: パススルーは**修飾付きチョード限定**（Ctrl/Alt/Cmd+key, Tab, F-keys 等）。素の印字キーは非対応。理由: (1) S の「ブラウザ KeyboardEvent → crossterm KeyEvent 再構築」は KeyEventKind(press/repeat)・IME composition(keyCode 229)・Option dead-key を正確に再現できない。(2) 修飾チョードは IME composition を起動しないため keydown-preventDefault の遮断保証が漏れなく成立する。

### D. 宣言場所 = Webview::register 静的（ホスト拡張あり）

オーナー承認の下、control-plane / wire / host を拡張する。
- SDK: `Webview::passthrough(keys: impl IntoIterator<Item = KeyChord>)` ビルダ（register 前に固定）。
- wire: `ClientMsg::Register` / `RegisterKind`（`sdk/.../protocol.rs` と `src/control_plane/protocol.rs`）に `passthrough_keys: Vec<...>` を追加。
- host: `DynamicView` に保持し、mount 時に glue へ**焼き込む**（`build_dynamic_preload` の context、または mount 直後の初期 emit）。これで「mount 後 push」の no-op タイミング脆弱性が構造的に消える。
- 動的更新が真に必要になった場合のみ、補助 `WebviewHandle::set_passthrough(keys)`（既存 emit 経路）を後付け（YAGNI、初版では不要）。

### E. API 一掃（ハード全廃）

オーナー裁定により最小サーフェス化。**削除**:
`FocusManager`, `FocusSync`, `Signal`, `focusable`, `NavKeymap`, `NavKey`, `Modifier`, `Direction`, `WebviewHandle::{set_nav_keys, set_page_focus, focus, blur, focus_instance}`, `keymap.rs` の `resolve_spatial` 等。

**残す/追加**:
- `WebviewWidget::{new, fallback, focused, is_focused}`（focused が同期駆動の唯一入口）。
- `Webview::{inline, dir, interactive, on, passthrough}` / `WebviewHandle::{id, emit}`。
- `KeyChord`（crossterm 直結）。
- `Ozma::{connect, register, frame, flush, try_event/events(/poll_event)}` + `Receiver<OzmaEvent>` 機構。
- 空間ナビ・フォーカスリングはアプリが自実装（SDK は提供しない）。クリック調停/ページ脱出 blur の「能力」は `OzmaEvent::FocusGained/Lost` 通知でアプリが自実装。

## 影響範囲（変更ファイル）

- **SDK (`sdk/ratatui-ozma/src/`)**: `focus.rs`/`keymap.rs` 削除、`session.rs`(OzmaEvent channel + try_event/flush focus差分), `webview.rs`(passthrough builder, focus/blur 等削除), `widget.rs`(focused を FramePlacements 記録へ), `protocol.rs`(Register に passthrough_keys, Focus op は flush 経由で残置), `lib.rs`(export 整理: KeyChord/OzmaEvent 追加、FocusManager 系削除)。
- **ホスト (`src/`)**: `control_plane/protocol.rs`(RegisterKind に passthrough_keys), `control_plane.rs`(DynamicView 保持 + build_view), `webview_render/preload.rs`+`ozmux_bridge.js`(passthrough チョード受領 + capture keydown 一般化 `__ozma.key`, DOM focus 報告), `inline_webview.rs`(passthrough を glue へ焼き込み)。`dispatch_focused_key`/bevy_cef は無改造。
- **例 (`sdk/ratatui-ozma/examples/`)**: `focus_grid.rs`/`ratatui_webview.rs` を新 API（widget::focused + Webview::passthrough + OzmaEvent merge ループ）へ書き換え。アプリ側で簡単なフォーカスリングを自実装するデモを含める。

## テスト計画

- SDK 単体: `KeyChord` serialize（glue 形状一致）、`Ozma` の OzmaEvent channel enqueue/try_event、flush の focus 差分（前フレーム比較で focus/blur op を送る/送らない）、passthrough builder の register wire。
- ホスト: register に passthrough_keys が載り `DynamicView` に保持され mount 時 glue へ届く経路、focus op の差分適用（既存 apply_control_events テスト流儀）。
- glue（substring/挙動）: capture keydown で宣言チョードを preventDefault+stopPropagation+`__ozma.key` 転送、DOM focus/blur 報告、素キーは素通し。
- 例: `cargo build --examples`。

## スコープ外 / 申し送り

- `Alt+h/j/k/l` がネイティブで届かない件（`encode_key` の Alt 未対応 / macOS Option 合成）は別タスク（本再設計はパススルー＝修飾チョード前提だが、ネイティブ側 Alt は別途）。
- 複数 mount instance のフォーカス（`focus_instance` 廃止）: 当面「1 handle = 1 visible instance」想定。multi-instance focus は将来別 API。
- H（host-tunnel）案: 不採用だが、将来「非協力ページ / 任意キー（修飾なし）対応」が必要になった場合の fallback として記録（bevy_cef filter + dispatch 例外）。

## Discussion Artifacts
- Team: ratatui-arch, ratatui-arch-cx, sdk-api, sdk-api-cx, host-cef, host-cef-cx
- Ratification: Round 1, 4/6 accept（push-back 2件はオーナー User Checkpoint で解決: Q1=register静的(ホスト拡張可), Q2=ハード全廃）
- Minority: ratatui-arch-cx（FocusManager 温存論）→ オーナー裁定で不採用、能力は OzmaEvent 通知で担保。
