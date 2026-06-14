# ratatui-ozma: フォーカス移動 & ナビゲーションキー・ルーティング設計

- 日付: 2026-06-14
- 対象: `sdk/ratatui-ozma`(SDK)、`src/control_plane`(ホスト)、`src/extension_render`(preload glue)、`src/inline_webview` / `src/extension_render`(FocusedWebview連携)
- 関連: [2026-06-14-ratatui-ozma-webview-design.md](2026-06-14-ratatui-ozma-webview-design.md)、[2026-06-14-altscreen-fixed-anchor-webview-design.md](2026-06-14-altscreen-fixed-anchor-webview-design.md)

## 背景と問題

`ratatui-ozma` は CEF オフスクリーン webview を ratatui ウィジェット
(`WebviewWidget`) として埋め込む SDK を提供する。現状、フォーカスは
**ホスト(ozmux GUI)所有**で、`bevy_cef` の `FocusedWebview`
(`Option<Entity>`) が唯一の真実。webview のクリック
(`set_focus_on_press`) かアクティブペイン追従
(`sync_focused_webview`) で設定される。

webview にフォーカスがある間、キー入力は CEF にネイティブ送信され、
ホストの `dispatch_focused_key` は PTY 転送を抑止する
(`src/input.rs`、`if focused_inline.is_some() { continue; }`)。
結果、**webview フォーカス中、ratatui アプリ(PTY 側 crossterm
イベントループ)はキーを一切受け取れない**。

達成したいこと:

1. ratatui アプリが、自分のペイン内のウィジェット群(ネイティブ
   ratatui ウィジェット + webview ウィジェットの混在)に対して持つ
   **独自のフォーカスリング**でフォーカスを移動できる。
2. webview にフォーカスがある間はキー入力を webview へ送るが、
   **予約ナビゲーションキー**(既定 `Alt+h/j/k/l`)は別ウィジェットへの
   フォーカス移動に使える。

## 設計判断の根拠(先行事例調査)

並列リサーチ(Codex + Claude Code Agent + Web)の結論:

- **WAI-ARIA / GTK / Qt / Slint / ターミナルマルチプレクサ(tmux,
  zellij, wezterm, kitty) / vim・Textual** のすべてが「素の矢印は中身の
  プログラム/ページへ、コンテナ間フォーカス移動は修飾キー/プレフィックス」
  で一致(approach C)。埋め込み webview = 1 つの tab stop(roving
  tabindex の発想)。
- **approach B(ホストが素のキーを CEF より前に横取り)は地雷**: CEF
  `OnPreKeyEvent` で素の矢印を奪うと IME 変換中の候補リスト操作
  (`keyCode===229` / `isComposing`)を壊す。実例 manaflow-ai/cmux
  PR #125(ターミナル+webview のキー横取りが CJK IME を破壊)。CEF OSR
  の `OnTakeFocus` も OSR では確実に呼ばれない(CEF #1826)。
- **approach A(ページ JS が境界検知)** は spatial-navigation 仕様
  (W3C css-nav-1 / WICG polyfill)で「テキスト欄のキャレットが端の時だけ
  矢印で脱出」として正統。ただし `contenteditable` はキャレット位置を
  露出できず汎用性に欠ける。
- **Codex の重要発見**: `bevy_cef` の `send_key_event` は同じ Bevy
  `KeyboardInput` を**独立に**読む(`MessageReader` は非消費型)。よって
  ozmux 側でキーを処理しても CEF にも届く。ホストがキーを「ページに
  見せず横取り」するには `bevy_cef` 側の配送抑止が必要(approach B の
  追加コスト)。
- inline focus 付与経路は**マウスクリックのみ**。アプリ起点で webview に
  フォーカスを与える control-plane op / SDK API は現状ゼロ。

採用: **「C で発火・A で配送」のハイブリッド(下記 approach 案1)。**
`bevy_cef` と `dispatch_focused_key` ホットパスは無改造。

## 全体アーキテクチャ: アプリ所有・ホスト追従

ratatui アプリがフォーカスリング(ネイティブ + webview 混在)を所有し、
常に 1 つがフォーカス中。ホストの `FocusedWebview` はアプリの指示に
追従する。

- **ネイティブにフォーカス**: `FocusedWebview=None`。キーは通常通り
  crossterm へ。アプリが `Alt+h/j/k/l` を解釈してリングを移動。
- **webview にフォーカス**: アプリが `focus{handle}` op をホストへ送る →
  ホストが `FocusedWebview` を該当 inline entity に設定 → CEF が
  ネイティブにキー受信(テキスト編集・IME 健全)。予約ナビキーだけ
  ページ glue 経由でアプリに戻り、フォーカス移動を駆動する。

### データフロー

```
[webview フォーカス中に Alt+l 押下]
  CEF → page keydown
    → SDK glue: 予約チョード一致 → preventDefault + stopPropagation
      → window.ozmux.call('__ozma.nav', ['right'])
        → control socket op:call → 登録元 ratatui アプリ
          → SDK 内蔵ハンドラ → チャネル → FocusManager.drain()
            → リング更新 → 次が webview なら focus{handle:Y} / ネイティブなら focus{handle:null}
              → control socket op:focus → ホスト apply_control_events
                → FocusedWebview 更新 → アプリ再描画
```

```
[ユーザーが webview をクリック / ホストの release_inline_focus]
  ホストが FocusedWebview を直接変更
    → CEF が page に focus/blur DOM イベント発火
      → SDK glue: window.ozmux.call('__ozma.focus', [bool])
        → ratatui アプリがリングを同期(新しい host→app op は不要)
```

## コンポーネント設計

### 1. コントロールプレーン・プロトコル追加(app→host)

唯一の新規 op。フォーカス付与/解放を 1 メッセージで表す。

```jsonc
{"op":"focus","handle":"<minted-handle>","instance":null}  // フォーカス付与
{"op":"focus","handle":null}                               // 解放(blur)
```

- **SDK 側** (`sdk/ratatui-ozma/src/protocol.rs`):
  `ClientMsg::Focus { handle: Option<String>, instance: Option<String> }`
  を追加。
- **ホスト側** (`src/control_plane/protocol.rs`):
  `ClientMsg::Focus { handle: Option<String>, instance: Option<String> }`
  を追加。listener が `ControlEvent::SetFocus { connection_id, handle,
  instance }` を発行。

`instance` は `(view_id, instance_id)` の既存 mount アドレスに対応し、
同一 view の複数インスタンスを区別する。

### 2. SDK 側の送出 API

- `WebviewHandle::focus(&self) -> OzmaResult<()>` — このハンドルに
  フォーカス付与(`Focus{handle:Some(id), instance:None}`)。
- `WebviewHandle::focus_instance(&self, instance: &str)` — 名前付き
  インスタンスへ。
- `Ozma::blur(&self) -> OzmaResult<()>` — `Focus{handle:None}` を送出
  (どの webview にもフォーカスを当てない = ネイティブへ)。

これらは既存の `SharedWriter` を使い、`emit` と同じ要領で 1 行書く。

### 3. ホスト側の focus 適用

`apply_control_events`(既存 Update システム)に `SetFocus` アームを追加:

1. 所有 surface の解決は **listener が既に接続ごとに解決している
   `owner_surface` を `ControlEvent::SetFocus` に載せて渡す**(新規マップ
   不要。`Register` が同様に `owner_surface` を threading 済み:
   `src/control_plane/listener.rs`)。`handle=None`(blur)も同じ
   `owner_surface` で解決できる。フォーカス対象 entity は
   `(&InlineWebview, &ChildOf, Has<NonInteractive>, Option<&WebviewOwner>)`
   で検索する。
2. **ガード(各拒否は `tracing::debug!` + return、既存 mount gate と
   同流儀)**:
   - 要求元 `connection_id` がそのハンドルを所有していること。
   - 解決 surface が要求元接続にバインドされた surface であること
     (他 surface のフォーカスを奪えない)。
3. そのsurfaceの生きた `InlineWebview` 子のうち
   `(view_id==handle, instance_id==instance)` 且つ interactive
   (非 `NonInteractive`)の entity を探す。未 mount / 非 interactive は
   拒否。
4. `FocusedWebview.0 = Some(entity)`(blur は `None`)。

`sync_focused_webview`(`src/extension_render.rs`)は既に「アクティブ
surface の子 inline にフォーカスがあれば保持」(`focused_inline_of`)する
ため、アプリ設定フォーカスは次フレームでクロバーされない。blur 時の
`None` も、ネイティブ surface は `WebviewSource` を持たないため
`sync_focused_webview` の `active` が `None` となり保持される。

### 4. ページ glue(SDK が preload で全 webview に自動注入)

ホストの preload(`src/extension_render/ozmux_bridge.js` 系。
`build_dynamic_preload` で全 dynamic webview に PreloadScripts として
注入済み)に小さな JS 層を追加。`window.ozmux` 上で動く。

責務:

1. **ナビキー横取り**: `keydown`(capture フェーズ)で予約チョード
   (既定 `Alt+h/j/k/l`)にマッチしたら `preventDefault()` +
   `stopPropagation()` し、`window.ozmux.call('__ozma.nav', [dir])`
   (dir = `left/down/up/right`)を送る。素の矢印・通常キー・修飾なしの
   入力は触らない(IME 安全・ページ自由)。
2. **フォーカス同期報告**: `window` の `blur`(CEF がフォーカスを外した
   = ホストの release chord / ペイン移動)と、クリック由来の `focus` を
   `window.ozmux.call('__ozma.focus', [bool])` でアプリへ通知。
3. **キーマップ受信**: アプリが `view.emit('__ozma.keys', set)` で予約
   キー集合を送ると glue が従う(アプリ側で設定可能)。`set` は
   `{mods, keys}` 形式の配列。

> **注意(未検証)**: 責務2の DOM `focus`/`blur` 報告は、CEF OSR で
> `FocusedWebview` 変更時に page の `window` focus/blur が確実に発火する
> 保証がない(CEF #1826 と同クラス)。よってリング同期の**一次手段は
> アプリ側 `FocusManager::drain` の整合チェック**(フォーカス対象 webview
> の生存・mount 状態の照合)とし、DOM イベント報告は補助・高速化に留める。
> 信頼できないと判明した場合のみ host→app の明示通知 op を将来追加する。

予約メソッド/イベントは `__ozma.` 名前空間。現状 `Webview::on`
(`sdk/ratatui-ozma/src/webview.rs`)は任意の method 文字列を受理し
衝突ガードが無いため、**SDK は `__ozma.*` をユーザー登録から予約**する
(`on("__ozma.*")` を debug_assert/エラーで拒否、内蔵ハンドラのみが使う)。

### 5. SDK Rust API(`FocusManager` + `WebviewWidget::focused`)

アプリ著者の利用イメージ:

```rust
let mut focus = FocusManager::new();
focus.add_native("editor");                    // ネイティブウィジェット id
focus.add_webview("search", search.clone());   // WebviewHandle
focus.add_native("status");

// イベントループ:
for _ in focus.drain(&ozma)? {                  // glue 由来(nav/focus報告)を反映
    // FocusManager が内部でリング更新 & focus/blur op を送出
}
if event::poll(timeout)? {
    if let Event::Key(k) = event::read()? {
        if focus.focused_is_native() {
            if let Some(dir) = FocusManager::nav_key(&k) {
                focus.navigate(dir, &ozma)?;    // ネイティブ間移動 + 必要なら focus op
            } else {
                // 著者がフォーカス中ネイティブウィジェットへキー処理
            }
        }
        // webview フォーカス中は crossterm にキーは来ない(CEF が保持)
    }
}

// 描画:
f.render_stateful_widget(
    WebviewWidget::new(search.id()).focused(focus.is_focused("search")),
    rect, ozma.frame());
```

- glue の `__ozma.nav` / `__ozma.focus` 呼び出しは、各 interactive
  Webview に**自動登録される内蔵 RPC ハンドラ**が受け、スレッド越え
  チャネル(`crossbeam_channel`)へ push。`FocusManager::drain()` が
  メインスレッドで吸い上げてリングへ適用。RPC ハンドラはリーダースレッド
  で動くため、`FocusManager` を直接触らずチャネル経由とする。
- `WebviewWidget::focused(bool)` を追加(フォーカス枠の描画ヒント。実体
  描画は CEF 側だが、枠/タイトルの強調に使う)。

`FocusManager` の責務:

- `add_native(id)` / `add_webview(id, handle)` — リング登録。
  ネイティブ矩形は `add_native_at` の手動再登録ではなく、webview 矩形と
  同じ frame 記録機構に寄せる(`FramePlacements` に `record_native(id,
  rect)` を追加し、描画時に記録)。「描画が幾何を記録」を単一機構に統一し
  二重管理を避ける。
- スレッド越えチャネルは既存依存の `crossbeam_channel`
  (`sdk/ratatui-ozma/src/session.rs` で既使用)を用いる。`rat-focus` は
  採用しない(線形 Tab 専用で空間 h/j/k/l 移動を扱わず、独自 widget 規約を
  公開 API に強いるため)。
- `navigate(direction, &ozma)` — 押下方向の最近傍へ移動。移動先が
  webview なら `focus{handle}`、ネイティブなら `blur` をホストへ送出。
- `drain(&ozma)` — glue 由来イベント(webview フォーカス中の nav 要求、
  クリック/解放の focus 報告)を吸い上げてリングへ適用。
- `is_focused(id) -> bool` / `focused_is_native() -> bool` —
  描画・分岐用。
- `nav_key(&KeyEvent) -> Option<Direction>`(関連関数) — 既定キーマップ
  解釈ヘルパ。

### 6. ナビキー・モデルと空間解決

- 既定予約キー: **`Alt+h/j/k/l`**(方向移動、設定可能)。ozmux 既定
  `Cmd+H/J/K/L`(ペイン間 `focus_pane`)と非衝突。
  `release_inline_focus`(Ctrl+Shift+Esc)はホストの最終脱出として温存。
- **空間解決**: h/j/k/l は方向。`FocusManager` はアイテムの矩形から押下
  方向の最近傍を選ぶ(spatial-navigation の最近傍アルゴリズム、アプリ側)。
  - `WebviewWidget` の矩形は描画時に `FramePlacements` に記録される
    (既存)。`FocusManager::drain` / 描画後にこれを読んで webview 矩形を
    得る。
  - ネイティブウィジェットの矩形は著者がレイアウト時に
    `add_native_at(id, rect)` で登録する(ratatui の動的レイアウトに
    追従)。
- 最近傍アルゴリズム(WICG polyfill / smart-TV LRUD 準拠):
  1. **半平面フィルタ**: 候補の近辺エッジが現フォーカス矩形の遠辺より
     押下方向に先行するものだけを残す(斜め隣接の誤選択を排除)。
  2. **重み付きコスト最小化**: `軸方向距離 + α·直交方向ずれ`(α≈0.3–0.5)
     を最小化。
  3. **安定タイブレーク**: 同コストは登録 index 最小を選ぶ(テスト決定性)。
  - データ構造は小さなフラット `Vec<FocusItem{id, rect, kind}>` の線形
    走査で十分(空間インデックスは不要)。

### 7. エッジケース / エラー処理

- **フォーカス中の webview が unmount / despawn**: ホストの
  `FocusedWebview` は entity 消滅で解決不能 → glue の `blur` 報告か、
  アプリ次回 `drain` 時の整合チェックで既定ネイティブへフォールバック。
- **接続切断**: `Disconnect` は登録解除と mounted inline webview の
  despawn を行うが、`FocusedWebview` を直接は触らない
  (`src/control_plane.rs`)。フォーカス中 entity の消滅後、後続の
  `sync_focused_webview` が stale focus を `None` へ収束させる。
- **複数 webview**: `(view_id, instance_id)` で識別(既存 mount
  アドレスと同じ)。
- **予約チョードと ozmux グローバル shortcut の衝突**: アプリは ozmux の
  global chord(既定 `Cmd+HJKL` 等)を避けたキーを選ぶ。`Alt+hjkl` は
  既定で非衝突。ozmux が先に `bindings.lookup` で奪うチョードは glue に
  届かない(ドキュメントで明記)。
- **非協力ページ**: glue 未注入なら予約ナビは効かない。ratatui-ozma
  では常時注入のため実害なし。`release_inline_focus` が最終脱出として
  常に効く。
- **focus op の競合(クリックと nav が同時)**: ホストは最後に受けた
  指示を適用(last-writer-wins)。glue の focus 報告でアプリが最終状態に
  収束。

## テスト計画

- **SDK 単体**(`sdk/ratatui-ozma`):
  - `protocol`: `Focus{handle:Some/None, instance}` のシリアライズ。
  - `FocusManager`: リング登録・空間解決(矩形固定の決定的ケース)・
    `navigate` の focus/blur op 送出・`drain` のチャネル反映。
  - 内蔵ハンドラ: `__ozma.nav` / `__ozma.focus` 呼び出しがチャネルへ
    正しく投入されること。
- **ホスト**(`src/control_plane` / `src/inline_webview` の `make_test_app`
  流儀):
  - `apply_control_events` の `SetFocus` アーム: 所有・interactive・
    mount 済みガード、`FocusedWebview` 設定、blur で `None`。
  - 所有しないハンドル / 非 interactive / 未 mount の拒否。
- **glue**(vitest):
  - keydown(予約チョード)→ preventDefault + `__ozma.nav` 呼び出し。
  - 素の矢印・通常キーは素通し。
  - window focus/blur → `__ozma.focus` 報告。
  - `__ozma.keys` 受信でキーマップ適用。

## スコープ外 / 将来拡張

- **境界認識の素の矢印**(spatial-nav 仕様式のキャレット端脱出): glue に
  opt-in で後付け可能な拡張点として確保。今回は既定 `Alt+hjkl` のみ。
- **非協力ページ向け approach 案2**(ホスト PTY トンネル + `bevy_cef`
  配送抑止): 将来 fallback として開けておく。
- **Tab/Shift+Tab の線形リング巡回**: `nav_key` の別マッピングとして
  容易に追加可能。
- **ホストの dormant な host-RPC 経由のフォーカス API**: 今回は
  control-plane 直結のみ。
