# tmux Pane ボーダー描画 & 隙間調整 — 設計

- 日付: 2026-06-16
- 対象ブランチ: `border`
- 関連クレート: `ozmux-gui`（`src/`）, `tmux_session`, `tmux_control_parser`, `ozma_tty_renderer`

## 背景と目的

ozmux は tmux control mode（`-CC`）をバックエンドに、各 tmux Pane を Bevy の絶対配置 `Node`（`MaterialNode<TerminalUiMaterial>`）として GPU 描画している。現状には 2 つの不満がある。

1. **Pane にボーダーが無い** — Pane の境界が視覚的に分からない。
2. **Pane 間の隙間が広すぎる** — tmux は隣接 Pane 間に **ちょうど 1 セルの境界セルを予約**する（例: `…{40x24,0,0,1,39x24,41,0,2}` では pane1 が `xoff=0,w=40`、pane2 が `xoff=41,w=39` で、列 40 が予約セル）。ozmux はこの予約セルを描画しないため、`cell_w`（横）/ `cell_h`（縦）ピクセルぶんがまるごと空白の隙間として見える。縦の隙間（`cell_h`）は横（`cell_w`）より大きく、特に目立つ。

ゴール: **各 Pane を 1px の境界線で仕切り、隙間を最小化する**。アクティブ Pane はアクセント色（青）の枠で強調、それ以外はグレーの線。

## 確定したデザイン

ビジュアル検討（VisualServer）で以下を確定:

- **方向**: Hairline（密着型）。Pane 同士は隙間 0px で接し、間を **1px の共有ライン**で仕切る。
- **アクティブ Pane**: アクセント青（`theme::ACCENT`）の 1px 枠で強調。
- **非アクティブ Pane / Pane 間の線**: グレー（`theme::BORDER`）。非アクティブ Pane は既存の `PaneDim` で減光済みのため、青枠はアクティブのみに付ける。
- **線幅**: 1px（`theme::PANE_BORDER_PX = 1.0`）。隙間 `GAP_PX = 1.0`。

## 現状アーキテクチャ（変更前）

- `tmux_control_parser::WindowLayout` が tmux のレイアウト文字列を再帰的な `Cell` ツリー（`Cell::Split { dir: SplitDir, children }` / `Cell::Leaf { dims, pane_id }`）にパース済み。
- `tmux_session` の projection（`events::pane_geoms` → `collect_leaves`）はツリーを**葉に潰して** `Vec<PaneGeom>` にし、各 Pane entity に `TmuxPane { id, dims: CellDims }` だけを持たせ、**ツリー構造は破棄**している。`%layout-change` は `observers::on_layout_changed` が処理。
- `src/tmux_render.rs::layout_tmux_panes` が各 `TmuxPane.dims` から `pane_rect(xoff, yoff, width, height, cell_w, cell_h)` でピクセル矩形を計算し、Pane の `Node`（`left/top/width/height`）に条件付き（値が変わった時だけ）で書き込む。グリッド（`TerminalGrid` の cols/rows）は tmux dims に一致させる。
- 端末描画は `ozma_tty_renderer` の `TerminalUiMaterial`（カスタム UI シェーダ）。**固定セルサイズ**（`cell_w_phys`×`cell_h_phys`）で `cols × rows` セルを描画し、Node は内容の実サイズちょうど。→ **Pane を平行移動して詰めるのは安全（内容はズレない）が、Node 矩形を膨らませると内容と矩形がズレる**。
- `theme.rs` に色・寸法が定義済み: `BORDER`（グレー, srgb 0.333）, `ACCENT`（青, srgb 0.302/0.561/0.851）, `PANE_BORDER_PX = 1.0`。
- アクティブ Pane は `ActivePane` マーカー（`tmux_session`）。非アクティブは `PaneDim`（`ozma_tty_renderer`）で減光。クリックフォーカスは `src/ui/tmux_pane_focus.rs`。

## 設計

### 1. レイアウトツリーの保持（`tmux_session`）

projection が捨てている `WindowLayout` ツリーを Window entity に保持する。

- `tmux_session/src/components.rs` に新コンポーネント `TmuxWindowLayout(pub WindowLayout)` を追加。
- **重要**: `pane_geoms` はイベント発火より前（`event_pump.rs:295,326`）でツリーを葉に潰すため、`on_layout_changed` は `WindowLayout` を受け取れない。よって `TmuxLayoutChanged` イベント（`events.rs`）の payload に `WindowLayout`（or root `Cell`）を追加し、`event_pump.rs` の構築2箇所でツリーを載せる。
- `observers::on_layout_changed` は受け取った `WindowLayout` を対応する `TmuxWindow` entity に `TmuxWindowLayout` として insert/update する。
- 葉→Pane 射影（`pane_geoms`）は従来通り維持。`TmuxPane.dims` も従来通り（後述の collapse は dims を直接は使わないが、グリッドサイズ計算で引き続き使用）。

### 2. 隙間を潰す純粋関数 `collapse`（`src/tmux_render.rs`）

ツリーを再帰的に歩き、**`xoff/yoff` は使わず**、各分割で子を順に「幅/高さ(セル) × cell_w/h」だけ進め、子の**間にだけ** `GAP_PX` を空ける。予約セル（1 セル = `cell_w`/`cell_h`px）がちょうど `GAP_PX`（1px）に潰れる。

```
// place は配置を out に書き、その subtree の packed 実寸 (w,h) を返す。
fn collapse(root: &Cell, cell_w: f32, cell_h: f32, gap: f32) -> HashMap<PaneId, Rect>  // 内部で place を呼ぶ
  place(cell, origin: Vec2, out) -> Vec2 /* packed size */:
    match cell:
      Leaf { dims, pane_id: Some(id) }:
        let size = (dims.width*cell_w, dims.height*cell_h)
        out[id] = Rect { origin: origin.round(), size: size.round() }   // ピクセルスナップ
        return size
      Leaf { pane_id: None }:           // パーサが pane_id 無しと判定した葉は配置しないが実寸は親へ返す
        return (dims.width*cell_w, dims.height*cell_h)
      Split { dir: LeftRight, children, .. }:
        x = origin.x; max_h = 0
        for (i, child) in children:
          let csz = place(child, vec2(x, origin.y), out)
          x += csz.x                    // tmux dims ではなく packed 実寸で送る
          max_h = max(max_h, csz.y)
          if i < last: x += gap
        return (x - origin.x, max_h)
      Split { dir: TopBottom, children, .. }:
        y = origin.y; max_w = 0
        for (i, child) in children:
          let csz = place(child, vec2(origin.x, y), out)
          y += csz.y                    // packed 実寸 + gap で送る
          max_w = max(max_w, csz.x)
          if i < last: y += gap
        return (max_w, y - origin.y)
      Split { dir: Floating, children, .. }:
        // ポップアップは浮く。潰さず各子を tmux の literal offset で配置。
        for child in children:
          place_literal(child, vec2(child.dims().xoff*cell_w, child.dims().yoff*cell_h), out)
        return (dims.width*cell_w, dims.height*cell_h)
```

- 各分割は自分のローカル座標系で処理され、兄弟の前進は **packed 実寸**（子 subtree の戻り値）で行う。これにより、ネストした subtree の後ろに予約セルぶんが復活する二重計上バグを除去。任意のネスト（LeftRight 内 TopBottom 等）で内部の継ぎ目が正確に 1px になる。
- 配置時に `.round()` でピクセルスナップし、DPR 由来（`cell_w=floor(advance_phys)/dpr` の分数）の累積誤差で隙間が 0/2px に化けるのを防ぐ。
- 純粋関数。`HashMap<PaneId, Rect>`（と root の packed bbox 実寸）を返す。ユニットテスト可能。

### 3. `layout_tmux_panes` の改修（`src/tmux_render.rs`）

フラットな `pane_rect` 計算を、Window ごとのツリー駆動 collapse に置き換える。

- クエリを `Window`（`&TmuxWindowLayout, &Children`）と Pane（`&TmuxPane, &mut Node, &mut TerminalHandle, &mut TerminalGrid`）に分割。
- 各 Window について `collapse(&layout.0.root, cell_w, cell_h, GAP_PX)` で packed 矩形マップを得る。
- Window の `Children` を走査し、`pane.id` で packed 矩形を引いて Pane の `Node`（`left/top/width/height`）に**条件付き書き込み**（既存パターン維持）。
- グリッドサイズ（cols/rows = `pane.dims`）の再計算・`resize_grid_only`・`emit_pending` は従来通り維持。Node 幅 = `dims.width*cell_w` のままなので内容はぴったり埋まりズレない。
- `cell_w`/`cell_h` の DPR 補正は既存ロジックを踏襲。

### 4. ライン描画 — packed bbox backdrop（`src/tmux_render.rs`）

Pane の背後に、collapse が返す **root の packed 実寸ちょうど**のサイズのグレー backdrop ノード（`BackgroundColor(theme::BORDER)`）を1枚敷く。packed Pane（不透明な端末描画）が 1px 隙間で並ぶため、その隙間から backdrop のグレーが透けて **1px の格子線**になる。線を1本ずつ Node で描く必要がない。**コンテナ全体（100%/100%）は塗らない** — collapse 後の総寸がウィンドウより小さくても、backdrop は packed 領域だけを覆うので右/下に数十pxのグレー帯が出ない。backdrop の外側は通常のアプリ背景。

### 5. アクティブ Pane の青枠（`Outline` コンポーネント）

- **第一候補**: アクティブ Pane entity に Bevy `Outline { width: Px(1), offset: Px(0), color: theme::ACCENT }` を insert、非アクティブ化で remove。ノード外側（1px 隙間上）に描かれ内容を隠さない。オーバーレイ entity・z順管理・毎フレーム矩形追従が不要。
- `ActivePane` の付け外しは既存の active 変化検知（`src/ui/tmux_pane_focus.rs` の `pane_active_state_changed`）に相乗りし、システムは `OzmuxTmuxRenderPlugin` に `layout_tmux_panes` の後で登録（別プラグイン `tmux_border.rs` は作らない）。
- **要スパイク**: `Outline` が `MaterialNode<TerminalUiMaterial>` 上で正しく描画され、隣接 Pane に遮蔽されないか確認（1px 隙間に乗るため遮蔽は起きにくい）。
- **フォールバック**（スパイク失敗時）: 膨らませた矩形（`left-1, top-1, width+2, height+2`）＋ `Node.border = UiRect::all(Px(1))` ＋ `BorderColor::all(theme::ACCENT)`（タプル形式 `BorderColor(..)` は Bevy 0.18 では不可）＋ `GlobalZIndex`（`src/ui/ime_overlay.rs` 先例）＋ `FocusPolicy::Pass` のオーバーレイ Node を `OzmuxTmuxRenderPlugin` に登録。

### 6. テーマ追加（`src/theme.rs`）

必要なら `PANE_GAP_PX: f32 = 1.0` を追加。色・線幅は既存（`BORDER`/`ACCENT`/`PANE_BORDER_PX`）を流用。

## エッジケース / 既知のトレードオフ

- **非対称ネスト**: 内部に継ぎ目を持つ subtree は、継ぎ目を持たない兄弟より packed 幅/高さが最大 `(cell − GAP)` ≈ 1 セルぶん狭くなり、外縁が僅かにズレる（固定セルサイズを保つ以上、回避不能な原理的トレードオフ）。対称な格子レイアウト（一般的）では完全整列。**許容**とする。
- **右/下の回収余白**: collapse 後の総 packed サイズはウィンドウより小さい（縮小量は外周経路上の継ぎ目数に依存し、単純な総継ぎ目数ではない）。backdrop を packed bbox ちょうどに敷く（§4）ため、余白部分はグレー帯にならず通常のアプリ背景になる。tmux に送る `refresh-client` の cols/rows 補正はしない（複雑さに見合わない）。
- **Floating/popup**（`SplitDir::Floating`）: 潰さず tmux の literal offset で配置。優先度低。
- **アクティブ Pane の遷移**: オーバーレイは毎フレーム packed 矩形を追従。`ActivePane` の付け外しに追従。

## テスト

- **`collapse` 純粋ユニットテスト**（`src/tmux_render.rs` の `#[cfg(test)]`）:
  - `…{40x24,0,0,1,39x24,41,0,2}`、cell 8×16、GAP=1 → pane1 `Rect(0,0,320,384)` / pane2 `Rect(321,0,312,384)`（40·8=320, 隙間 1 → 321, 39·8=312, 高さ 24·16=384）。
  - ネスト（LeftRight 内 TopBottom）での子の packed 位置。
  - 単一 Pane → 隙間なしで全面。
  - `pane_id: None` の葉がスキップされること。
- 既存テスト（`pane_rect_scales_cell_dims_to_pixels` 等）は、collapse へ移行する範囲に合わせて追従・置換。
- `tmux_session` 側: `on_layout_changed` が `TmuxWindowLayout` を Window に付けることのテスト。

## 変更ファイル一覧

- `crates/tmux_session/src/components.rs` — `TmuxWindowLayout` コンポーネント追加。
- `crates/tmux_session/src/events.rs` — `TmuxLayoutChanged` payload に `WindowLayout`（or root `Cell`）を追加。
- `crates/tmux_session/src/event_pump.rs` — `TmuxLayoutChanged` 構築2箇所（`:295,326`）でツリーを載せる。
- `crates/tmux_session/src/observers.rs` — `on_layout_changed` で受け取った `WindowLayout` を `TmuxWindowLayout` として insert/update。
- `src/tmux_render.rs` — `collapse`（packed 実寸を返す）追加、`layout_tmux_panes` をツリー駆動に改修、packed bbox backdrop ノード、アクティブ Pane の `Outline` システムを `OzmuxTmuxRenderPlugin` に登録。
- `src/theme.rs` — 必要なら `PANE_GAP_PX` 追加。
