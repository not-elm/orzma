# placement の所有権を `Screen` へ降ろす — 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** webview placement の表を `PlacementStore`（端末スコープの単一テーブル）から解体し、表そのものを `Screen` が所有する形へ移す。あわせて placement 座標を viewport 空間から grid 空間へ移し、eviction をチャンク末尾の両スクリーン sweep に一本化する。

**Architecture:** 表を `ScreenPlacements`（`screen/placements.rs`）として per-screen 化し、`Screen` の 8 番目のフィールドにする。端末スコープの不変条件 3 つ（`PlacementId` の採番、`MAX_PLACEMENTS` の cap、`(view_id, instance_id)` のアドレス空間）だけを `DeviceState` が引き取る。anchor の解決は `Grid::grid_line` 一箇所に集約し、射影と sweep が同一の式を共有する。`ActiveScreen` 型は役目を失って消える。

**Tech Stack:** Rust edition 2024 / toolchain 1.95、Bevy 0.19（`orzma_tty_renderer` / `orzma_webview` 側）。テストは `cargo test`。

**Spec:** `docs/superpowers/specs/2026-08-26-placement-ownership-design.md`

## Global Constraints

- **コメント言語は英語。** `//`・`///`・`//!` すべて。会話が日本語でも例外なし（`CLAUDE.md` の "Comment language"）。
- **コメント分類は 3 種のみ:** `// TODO:` / `// NOTE:` / `// SAFETY:`。ナラティブコメント、ブロックコメント、コメントアウトされたコードは禁止（`.claude/rules/rust.md`）。
- **`// NOTE:` は critical caveat 専用。** 見落とすとバグ・クラッシュ・データ損失・不変条件違反が起きる場合に限る。「非自明」「知っておくと良い」は不可。
- **doc コメント:** 外部 `pub` の item は `///` 必須。`pub(crate)` は不要（推奨）。ファイルレベルのモジュールは `//!` 必須。
- **コメント散文は完全な英文。** 電文体の断片は不可。1 段落 1〜3 文に収める。
- **`#[test]` の doc:** 1 行目に主張、空行、`Case:` 段落のみ。`Case:` は想定シナリオだけを書き、アサーションの言い換え・ポリシー・仮想の壊れた実装の挙動は書かない。決まったポリシーは 1 行目に畳む。
- **import はファイル冒頭に 1 ブロック。** `std` / 外部 crate / crate 内で空行を入れない。インラインのフルパス禁止（`use` を足す）。`#[cfg(test)] mod tests` 内のローカル `use` だけ例外。
- **`mod.rs` 禁止。** `foo.rs` + `foo/bar.rs`。
- **item 順序:** `pub` → `pub(crate)` → private。private helper は impl ブロックの末尾。
- **引数順序:** mutable な引数を immutable より先に。
- **`Query` 引数に `_q` サフィックス禁止。**
- **`#[expect(..., reason = "...")]` を `#[allow]` より優先。**
- **可視性:** `orzma_vt` の `mod` は `lib.rs` で無修飾なので、メソッドと関連定数は `pub` で書く（`pub(crate) fn` を重ねない）。既存の `PlacementStore` / `DeviceState` がその形。
- **検証コマンドはワークスペース全体ではなく 3 クレート指定で走らせる。** `orzma_tty_engine` はこのブランチで既に 32 件のコンパイルエラーを抱えており（`FrameSnapshot` / `ViCursor` / `SelectionKind` のフィールド不一致）、`cargo check --workspace` は通らない。本計画が触るのは `orzma_vt` / `orzma_tty_renderer` / `orzma_webview` の 3 つだけなので、`cargo test -p orzma_vt -p orzma_tty_renderer -p orzma_webview` と `cargo clippy -p orzma_vt -p orzma_tty_renderer -p orzma_webview --all-targets --fix --allow-dirty --allow-staged && cargo fmt` を各タスクのコミット前に通す。
- **clippy は「新しい警告が増えていないこと」を見る。** `orzma_vt` は着手前から 3 件の警告を出しているので、ゼロにはならない。

---

## File Structure

| ファイル | 責務 | 変更 |
| - | - | - |
| `crates/orzma_vt/src/placement.rs` | 公開語彙（`PlacementId` / `PlacementSize` / `AnchoredPlacement`）と `MAX_PLACEMENTS` **だけ**が残る | 型の再形成 → 最終的に `PlacementStore` / `Placement` を削除 |
| `crates/orzma_vt/src/screen/placements.rs` | **新規。** 1 スクリーン分の mount 表・射影・sweep | 新設 |
| `crates/orzma_vt/src/screen.rs` | 8 番目のフィールド `placements` と 14 番目の impl ブロック | 追加 + `viewport_row_of` の削除 |
| `crates/orzma_vt/src/device.rs` | 端末スコープの 3 つ（採番・cap・アドレス空間）と両スクリーン sweep | 追加 + `ActiveScreen` / `active_screen()` の削除 |
| `crates/orzma_vt/src/frame.rs` | `emit` の第 2 引数を落とし、射影を `device.active()` から取る | 変更 |
| `crates/orzma_vt/src/interpreter.rs` | `Executor` の借用フィールドが 6 → 5 | 変更 |
| `crates/orzma_vt/src/lib.rs` | `OrzmaVt::placements` フィールドの削除、prelude の型名差し替え | 変更 |
| `crates/orzma_vt/src/screen/viewport.rs` | `ViewportLine` の doc から stale な負値・センチネル記述を落とす | doc のみ |
| `crates/orzma_tty_renderer/src/schema.rs` / `schema/grid.rs` / `schema/frame.rs` / `grid.rs` | 再エクスポートと `TerminalGrid.placements` / `FrameSnapshot.placements` / `FrameDelta.placements` の型名 | 変更 |
| `crates/orzma_webview/src/webview/mount.rs` | `project_webview_overlays` が `display_offset` を適用する | 変更 |

---

## Task 1: placement を grid 空間で出す（`AnchoredPlacement`）

所有権の移動とは独立した先行変更。フレームが運ぶ placement 座標を viewport 空間から grid 空間へ移し、`Cursor` / `SelectionRange` と同じ規約に揃える。この段階だけで木が通る。

**Files:**
- Modify: `crates/orzma_vt/src/placement.rs:1-7`（module doc）, `:24-42`（型）, `:88-103`（`project`）, `:176-183`（`evict_lost_anchors`）
- Modify: `crates/orzma_vt/src/screen.rs:607-618`（`viewport_row_of`）, `:621-625`（impl ブロック doc）, `:2260-2307`（テスト）
- Modify: `crates/orzma_vt/src/device.rs:171-175`（`ActiveScreen::viewport_row_of`）
- Modify: `crates/orzma_vt/src/lib.rs:39`（prelude）
- Modify: `crates/orzma_vt/src/frame.rs:51-53`（`Frame.placements` doc）, `:17`, `:54`, `:87`, `:191`, `:214`
- Modify: `crates/orzma_vt/src/screen/viewport.rs:18-30`（`ViewportLine` doc と struct）
- Modify: `crates/orzma_tty_renderer/src/schema.rs:12-16`, `crates/orzma_tty_renderer/src/schema/grid.rs:3`, `:78-82`, `crates/orzma_tty_renderer/src/schema/frame.rs:2`, `:29-31`, `:58-60`, `crates/orzma_tty_renderer/src/grid.rs:345-350`
- Test: `crates/orzma_webview/src/webview/mount.rs`（`mod tests`）

**Interfaces:**
- Consumes: なし（最初のタスク）
- Produces:
  - `pub struct AnchoredPlacement { pub id: PlacementId, pub point: GridPoint, pub size: PlacementSize }` — `ProjectedPlacement` を置き換える
  - `Screen::grid_line_of(&self, id: LineId) -> Option<GridLine>` —— 既存 `pub(crate) fn viewport_row_of` の改名・再型付け。`device.rs` は `Screen` の private な `grid` に届かないので `ActiveScreen` が生きている間はアクセサが要る。Task 5 で `ActiveScreen` ごと消える（4 行）。**Global Constraint に従い `pub` で書く** —— 現行の `pub(crate)` は spec が「例外は残らない」と書いた当のものなので、ここで持ち越さない
  - `ActiveScreen::grid_line_of(&self, id: LineId) -> Option<GridLine>`（**Task 5 で `ActiveScreen` ごと削除**）
  - `PlacementStore::project(&self, active: ActiveScreen<'_>) -> Vec<AnchoredPlacement>`

- [ ] **Step 1: 失敗するテストを書く（消費側の新しい契約）**

`crates/orzma_webview/src/webview/mount.rs` の `mod tests` 内、既存の `grid_with_placements` の直後にヘルパを足し、既存 `grid_with_placements` をそれに委譲させる。

```rust
    fn grid_with_placements(
        rows: u16,
        cols: u16,
        placements: Vec<AnchoredPlacement>,
    ) -> TerminalGrid {
        grid_with_placements_at(rows, cols, 0, placements)
    }

    fn grid_with_placements_at(
        rows: u16,
        cols: u16,
        display_offset: u32,
        placements: Vec<AnchoredPlacement>,
    ) -> TerminalGrid {
        TerminalGrid {
            rows,
            cols,
            display_offset,
            placements,
            ..Default::default()
        }
    }
```

`projection_culls_fully_outside_rects` の隣に新しいテストを足す。これは `crates/orzma_vt/src/screen.rs:2278` の `scrolling_back_moves_an_anchors_reported_row_down` が固定していた契約の引っ越し先である。

```rust
    /// Asserts that a scrolled viewport moves a placement's rect down by
    /// the display offset.
    ///
    /// Case: the user scrolls the terminal back over output that carries a
    /// mounted webview, and the rect has to stay on the text it was
    /// anchored to.
    #[test]
    fn a_scrolled_viewport_moves_the_rect_down() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_orzma(&mut app, "memo", terminal, true);
        mount(&mut app, terminal, "memo", Some(PlacementId(1)));

        app.world_mut()
            .entity_mut(terminal)
            .insert(grid_with_placements_at(
                24,
                80,
                3,
                vec![AnchoredPlacement {
                    id: PlacementId(1),
                    point: GridPoint {
                        line: GridLine(-2),
                        column: GridColumn(0),
                    },
                    size: PlacementSize { rows: 6, cols: 10 },
                }],
            ));
        run_projection(&mut app);
        assert_eq!(overlays_of(&app, terminal).rects[0].x, 1);
    }
```

`mod tests` の `use` に `AnchoredPlacement`, `GridLine`, `GridPoint` を足す（既存の `use` ブロック `crates/orzma_webview/src/webview/mount.rs:670-675` に追記し、`ProjectedPlacement` を外す）。

- [ ] **Step 2: テストを走らせてコンパイルエラーになることを確認する**

Run: `cargo test -p orzma_webview a_scrolled_viewport_moves_the_rect_down`
Expected: FAIL — `cannot find type AnchoredPlacement in this scope` / `no field point on type ProjectedPlacement`

- [ ] **Step 3: 公開型を grid 空間へ作り替える**

`crates/orzma_vt/src/placement.rs:24-42` の `ProjectedPlacement` を置き換える。

```rust
/// One placement's grid-space geometry at emit time.
///
/// The point is in active-grid coordinates and does not move when the
/// user scrolls, the same way a cursor point or a selection endpoint
/// does not; the consumer projects it with the frame's display offset.
///
/// # Invariants
///
/// `size` always equals the mount-time reservation for `id`; the VT
/// treats a size change as a remount under a fresh id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchoredPlacement {
    /// The placement this geometry belongs to.
    pub id: PlacementId,
    /// Active-grid cell the rect's top-left corner sits at.
    pub point: GridPoint,
    /// The rect's extent, unchanged from the mount that reserved it.
    pub size: PlacementSize,
}
```

同ファイル冒頭の module doc（`:1-7`）を書き換える。

```rust
//! Webview placement table: id minting, anchor tracking, and the
//! grid-space geometry an emitted frame carries.
//!
//! [`PlacementStore`] is a side table keyed by the grid row a mount
//! anchored to, never a cell variant, so text writes and reflow cannot
//! corrupt a placement.
```

`use` を差し替える: `use crate::screen::grid::coords::GridColumn;` → `use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint};`

- [ ] **Step 4: `Screen::viewport_row_of` を `grid_line_of` に置き換える**

`crates/orzma_vt/src/screen.rs:606-618` を置き換える。

```rust
    /// The active-grid line `id`'s row now sits at; `None` once the row
    /// has left the ring.
    pub fn grid_line_of(&self, id: LineId) -> Option<GridLine> {
        self.grid.grid_line(id)
    }
```

10 番目の impl ブロック（`/// The viewport the user sees.`）ではなく、そのままの位置に残してよい。11 番目のブロック doc（`:621-625`）の `viewport_row_of` への言及を落とす。

```rust
/// What an emitted frame reads back.
///
/// The damage projection lives here because it converts screen rows into
/// the viewport coordinates a frame repaints by.
```

- [ ] **Step 5: `screen.rs` の `mod viewport_row_of` を畳む**

`crates/orzma_vt/src/screen.rs:2260-2306` の `mod viewport_row_of` を丸ごと削除する。3 本の行き先は次のとおり。

| テスト | 行き先 |
| - | - |
| `an_anchor_above_the_viewport_reports_a_negative_row` | 削除。`crates/orzma_vt/src/screen/grid.rs:434` の `grid_line` テストが同じことを固定している |
| `an_anchor_trimmed_from_the_ring_stops_resolving` | 削除。`screen/grid.rs:449` 近傍の trim テストが同じ |
| `scrolling_back_moves_an_anchors_reported_row_down` | Step 1 の `a_scrolled_viewport_moves_the_rect_down` が引き継ぐ |

- [ ] **Step 6: `ActiveScreen` のアクセサを差し替える**

`crates/orzma_vt/src/device.rs:171-175` を置き換える。

```rust
    /// The active-grid line `id` now sits at; `None` once the row has
    /// left the ring.
    pub fn grid_line_of(&self, id: LineId) -> Option<GridLine> {
        self.screen.grid_line_of(id)
    }
```

`crates/orzma_vt/src/device.rs` の `use` ブロックに `GridLine` を足す: `use crate::screen::grid::coords::GridColumn;` → `use crate::screen::grid::coords::{GridColumn, GridLine};`

- [ ] **Step 7: `PlacementStore` の射影と sweep を grid 空間にする**

`crates/orzma_vt/src/placement.rs:88-103` の `project` の本体を置き換える。

```rust
    pub fn project(&self, active: ActiveScreen<'_>) -> Vec<AnchoredPlacement> {
        self.placements
            .iter()
            .filter(|p| p.screen == active.kind())
            .filter_map(|p| {
                Some(AnchoredPlacement {
                    id: p.id,
                    point: GridPoint {
                        line: active.grid_line_of(p.anchor)?,
                        column: p.col,
                    },
                    size: p.size,
                })
            })
            .collect()
    }
```

同 doc の最後の段落を書き換える。

```rust
    /// A placement whose anchor has left the ring is omitted rather than
    /// removed, so it stays invisible but alive until the next eviction
    /// sweep. Nothing is culled here: a point outside the viewport passes
    /// through for the consumer to clip.
```

`:176-183` の `evict_lost_anchors` の述語を差し替える。

```rust
        self.evict_where(|p| {
            p.screen == active.kind() && active.grid_line_of(p.anchor).is_none()
        })
```

- [ ] **Step 8: `Frame` と prelude の型名を差し替える**

- `crates/orzma_vt/src/lib.rs:39` の `ProjectedPlacement` を `AnchoredPlacement` に
- `crates/orzma_vt/src/frame.rs:17`, `:54`, `:87`, `:191`, `:214` の `ProjectedPlacement` を `AnchoredPlacement` に
- `crates/orzma_vt/src/frame.rs:50-53` の `Frame.placements` doc を書き換える

```rust
    /// Webview placements in active-grid coordinates: `None` when
    /// unchanged since the last emitted frame, otherwise the complete
    /// list — `Some(vec![])` means no placement has a live anchor, which
    /// is not an unmount. The consumer projects each point with
    /// `display_offset` and culls what falls outside the viewport.
```

- [ ] **Step 9: `ViewportLine` の stale な doc を落とす**

`crates/orzma_vt/src/screen/viewport.rs:18-27` を置き換える。負値と `-1` センチネルの記述は `orzma_tty_engine` のワイヤ型（`ViCursor.row: i16` / `ViewportPoint.row: i16`）の規約であって、`u16` のこの型には当てはまらない。

```rust
/// A line in viewport coordinates: `0` is the topmost visible row.
///
/// It is the viewport projection of a [`crate::prelude::GridLine`],
/// related by `viewport_line = grid_line + display_offset`, and only
/// rows the viewport actually shows are representable. The ordering is
/// spatial — where the row sits in the window this frame — not an
/// identity: the same `ViewportLine` names different content once the
/// user scrolls or the grid is resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ViewportLine(pub u16);
```

- [ ] **Step 10: レンダラ側の再エクスポートとテストリテラルを直す**

- `crates/orzma_tty_renderer/src/schema.rs:12-16` の `ProjectedPlacement` を `AnchoredPlacement` に（`GridPoint` は既に再エクスポート済み）
- `crates/orzma_tty_renderer/src/schema/grid.rs:3` の import、`:78-82` の `TerminalGrid.placements` の型と doc

```rust
    /// Webview placements in active-grid coordinates, mirrored from the
    /// last applied frame. Replaced wholesale on snapshot AND delta —
    /// absence from the list means "no live anchor this frame".
    pub placements: Vec<AnchoredPlacement>,
```

- `crates/orzma_tty_renderer/src/grid.rs:345-350` のテストリテラル

```rust
        let placed = AnchoredPlacement {
            id: PlacementId(1),
            point: GridPoint {
                line: GridLine(2),
                column: GridColumn(3),
            },
            size: PlacementSize { rows: 4, cols: 5 },
        };
```

- `crates/orzma_tty_renderer/src/schema/frame.rs:2` の import、`:29-31` の `FrameSnapshot.placements`、`:58-60` の `FrameDelta.placements` —— **`schema.rs` の再エクスポートだけを直すとここでコンパイルが壊れる。** 型名と、doc の "Viewport-projected webview placements" を書き換える

```rust
    /// Webview placements in active-grid coordinates — the complete list
    /// for this frame; the consumer projects each point with
    /// `display_offset`.
    pub placements: Vec<AnchoredPlacement>,
```

`crates/orzma_tty_renderer/src/grid.rs:133` の test `use` に `AnchoredPlacement` を足し `ProjectedPlacement` を外す。`GridLine` と `GridPoint` は `grid.rs:4-7` の top-level `use` 経由で `mod tests` の `use super::*` から既に見えているので追加不要。

- [ ] **Step 11: `project_webview_overlays` に display offset を適用する**

`crates/orzma_webview/src/webview/mount.rs:603-618` を置き換える。

```rust
                let row = i64::from(projected.point.line.0) + i64::from(grid.display_offset);
                if row + i64::from(projected.size.rows) <= 0
                    || row >= i64::from(grid.rows)
                    || u32::from(projected.point.column.0) >= u32::from(grid.cols)
                {
                    continue;
                }
                let slot = usize::from(view.slot);
                if slot >= OVERLAY_SLOTS {
                    continue;
                }
                let row = i32::try_from(row).expect("the cull above bounds the row to the viewport");
                overlays.rects[slot] = IVec4::new(
                    row,
                    i32::from(projected.point.column.0),
                    i32::from(projected.size.rows),
                    i32::from(projected.size.cols),
                );
```

同関数の doc（`:558-566`）の "a negative `viewport_row` passes through" を書き換える。

```rust
/// The list is authoritative and declarative: an id with no matching
/// child is ignored (its mount signal has not landed yet), a mounted
/// child whose id is absent paints nothing (hidden, not unmounted), and
/// a rect whose top sits above the viewport passes through with a
/// negative row for the shader to clip. Each point is projected with the
/// grid's display offset; rects fully outside the viewport and columns
/// at or past the right edge are culled here.
```

`mount.rs:56` の `ProjectedPlacement.size` への言及を `AnchoredPlacement.size` に直す。

- [ ] **Step 12: `mount.rs` の残りのテストリテラルを直す**

`placed()`（`:778-785`）と、`:1393`, `:1414`, `:1435`, `:1468`, `:2291`, `:2343`, `:2361`, `:2424` の `ProjectedPlacement { .., viewport_row: N, col: GridColumn(C), .. }` を `AnchoredPlacement { .., point: GridPoint { line: GridLine(N), column: GridColumn(C) }, .. }` に置き換える。`display_offset` は既定の 0 なので、行の値はそのままでよい。

```rust
    /// The canonical 10x40 frame-carried rect at grid line 2, column 3
    /// the projection tests share.
    fn placed(id: PlacementId) -> AnchoredPlacement {
        AnchoredPlacement {
            id,
            point: GridPoint {
                line: GridLine(2),
                column: GridColumn(3),
            },
            size: PlacementSize { rows: 10, cols: 40 },
        }
    }
```

- [ ] **Step 13: 全テストを走らせる**

Run: `cargo test -p orzma_vt -p orzma_tty_renderer -p orzma_webview`
Expected: PASS（`a_scrolled_viewport_moves_the_rect_down` を含む全件）。`--workspace` は使わない —— `orzma_tty_engine` が着手前から壊れている

- [ ] **Step 14: lint と format**

Run: `cargo clippy -p orzma_vt -p orzma_tty_renderer -p orzma_webview --all-targets --fix --allow-dirty --allow-staged && cargo fmt`
Expected: `orzma_vt` の既存 3 件以外に新しい警告が出ないこと

- [ ] **Step 15: コミット**

```bash
git add -A
git commit -m "refactor(orzma_vt): emit placements in grid space"
```

---

## Task 2: `screen/placements.rs` — 1 スクリーン分の表

新しい表を実装し、`placement.rs` の表操作テストを移植する。この時点では誰も使わないので、木は通るが production の読み手はいない。

**Files:**
- Create: `crates/orzma_vt/src/screen/placements.rs`
- Modify: `crates/orzma_vt/src/screen.rs`（`mod placements;` の宣言を足す）

**Interfaces:**
- Consumes: Task 1 の `AnchoredPlacement`
- Produces:
  - `pub(crate) struct ScreenPlacements`
  - `ScreenPlacements::new() -> Self`
  - `ScreenPlacements::len(&self) -> usize`
  - `ScreenPlacements::is_empty(&self) -> bool`
  - `ScreenPlacements::mount(&mut self, id: PlacementId, anchor: LineId, col: GridColumn, size: PlacementSize, view_id: String, instance_id: Option<String>)`
  - `ScreenPlacements::supersede(&mut self, view_id: &str, instance_id: Option<&str>)`
  - `ScreenPlacements::unmount(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool`
  - `ScreenPlacements::project(&self, line_of: impl Fn(LineId) -> Option<GridLine>) -> Vec<AnchoredPlacement>`
  - `ScreenPlacements::evict_lost_anchors(&mut self, line_of: impl Fn(LineId) -> Option<GridLine>) -> Vec<PlacementId>`
  - `ScreenPlacements::take_all(&mut self) -> Vec<PlacementId>`

- [ ] **Step 1: モジュールを宣言し、失敗するテストを書く**

まず `crates/orzma_vt/src/screen.rs` のモジュール宣言に足す（既存の `pub(crate) mod` 群の並びに合わせる）。**これを Step 3 まで遅らせてはいけない** —— 宣言の無いファイルはビルドに含まれず、Step 2 のコマンドが 0 件選択して素通りしてしまい、赤にならない。

```rust
pub(crate) mod placements;
```

次に `crates/orzma_vt/src/screen/placements.rs` を新規作成し、テストだけ書く。

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::grid::Grid;
    use crate::screen::grid::GridSize;
    use crate::screen::grid::coords::ScreenLine;

    fn table() -> ScreenPlacements {
        ScreenPlacements::new()
    }

    fn grid() -> Grid {
        Grid::new(GridSize { cols: 8, rows: 3 }, 10)
    }

    fn mount(table: &mut ScreenPlacements, grid: &Grid, id: u64, view: &str) {
        table.mount(
            PlacementId(id),
            grid.line_id(ScreenLine::TOP),
            GridColumn(0),
            PlacementSize { rows: 2, cols: 4 },
            view.to_string(),
            None,
        );
    }

    /// Asserts that a re-mount of the same address replaces the live
    /// placement instead of stacking a second one beside it.
    ///
    /// Case: a program re-renders the same named view after its content
    /// changed.
    #[test]
    fn a_supersede_replaces_the_live_placement_at_the_same_address() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1, "memo");
        table.supersede("memo", None);
        mount(&mut table, &grid, 2, "memo");
        assert_eq!(table.len(), 1);
        assert_eq!(table.take_all(), vec![PlacementId(2)]);
    }

    /// Asserts that an unmount naming only a view id removes every
    /// placement at that view, and that a `None` view id removes all.
    ///
    /// Case: a program tears down one of its views, then exits and asks
    /// the terminal to drop whatever is left.
    #[test]
    fn an_unmount_removes_the_placements_its_address_names() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1, "memo");
        mount(&mut table, &grid, 2, "chart");
        assert!(table.unmount(Some("memo"), None));
        assert_eq!(table.len(), 1);
        assert!(!table.unmount(Some("memo"), None));
        assert!(table.unmount(None, None));
        assert!(table.is_empty());
    }

    /// Asserts that projection omits the placements the resolver rejects
    /// and pairs the resolved line with the mount-time column.
    ///
    /// Case: a frame is emitted while one of two mounted webviews has
    /// scrolled out of the grid ring entirely.
    #[test]
    fn a_projection_omits_what_the_resolver_rejects() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1, "memo");
        let projected = table.project(|_| Some(GridLine(-4)));
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, PlacementId(1));
        assert_eq!(projected[0].point.line, GridLine(-4));
        assert_eq!(projected[0].point.column, GridColumn(0));
        assert!(table.project(|_| None).is_empty());
    }

    /// Asserts that the sweep drops exactly the placements the resolver
    /// rejects and names them.
    ///
    /// Case: a chunk of output scrolls one webview's anchor row out of
    /// the ring while another stays put.
    #[test]
    fn a_sweep_drops_and_names_the_unresolvable_placements() {
        let mut table = table();
        let grid = grid();
        mount(&mut table, &grid, 1, "memo");
        assert!(table.evict_lost_anchors(|_| Some(GridLine(0))).is_empty());
        assert_eq!(table.evict_lost_anchors(|_| None), vec![PlacementId(1)]);
        assert!(table.is_empty());
    }

    /// Asserts that an empty table resolves no anchor at all.
    ///
    /// Case: a chunk ends on a screen that has never held a webview, and
    /// the sweep runs anyway.
    #[test]
    fn an_empty_table_resolves_no_anchor() {
        let mut table = table();
        let mut resolved = 0;
        let swept = table.evict_lost_anchors(|_| {
            resolved += 1;
            None
        });
        assert!(swept.is_empty());
        assert_eq!(resolved, 0);
    }
}
```

- [ ] **Step 2: テストを走らせて失敗を確認する**

Run: `cargo test -p orzma_vt screen::placements`
Expected: FAIL — `cannot find type ScreenPlacements in this scope`（モジュールは Step 1 で宣言済みなので、欠けているのは実装本体だけ）

- [ ] **Step 3: 実装を書く**

`crates/orzma_vt/src/screen/placements.rs` のテストモジュールの上に置く。

```rust
//! The webview placements one screen owns: the mount table, the
//! resolution of its anchors into grid coordinates, and the eviction
//! sweep.

use crate::placement::{AnchoredPlacement, PlacementId, PlacementSize};
use crate::screen::grid::LineId;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint};

/// The placements mounted on one screen.
///
/// Which screen owns the table answers which screen a placement belongs
/// to, so no placement records its own screen.
#[derive(Debug)]
pub(crate) struct ScreenPlacements {
    placements: Vec<Placement>,
}

impl ScreenPlacements {
    /// Builds an empty table.
    pub fn new() -> Self {
        Self {
            placements: Vec::new(),
        }
    }

    /// Number of placements this screen holds.
    pub fn len(&self) -> usize {
        self.placements.len()
    }

    /// Whether this screen holds no placement.
    ///
    /// [`Self::evict_lost_anchors`] returns on this before resolving a
    /// single anchor, which is what keeps sweeping a screen with no
    /// webview free.
    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// Registers a mount; the caller has already minted `id` and resolved
    /// the anchor.
    pub fn mount(
        &mut self,
        id: PlacementId,
        anchor: LineId,
        col: GridColumn,
        size: PlacementSize,
        view_id: String,
        instance_id: Option<String>,
    ) {
        self.placements.push(Placement {
            id,
            anchor,
            col,
            size,
            view_id,
            instance_id,
        });
    }

    /// Drops the placement a re-mount replaces, without reporting it.
    ///
    /// A superseded id is deliberately unnamed: the host re-points the
    /// same entity at the successor and would despawn it if the
    /// superseded id were reported as evicted.
    pub fn supersede(&mut self, view_id: &str, instance_id: Option<&str>) {
        self.placements
            .retain(|p| !p.addressed_by(view_id, instance_id));
    }

    /// Removes the placements a client `unmount` addresses; returns
    /// whether anything went.
    pub fn unmount(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool {
        let before = self.placements.len();
        self.placements.retain(|p| match (view_id, instance_id) {
            (None, _) => false,
            (Some(view), None) => p.view_id != view,
            (Some(view), Some(instance)) => !p.addressed_by(view, Some(instance)),
        });
        before != self.placements.len()
    }

    /// Resolves every placement's anchor through `line_of` — the complete
    /// list, not a diff.
    ///
    /// # Invariants
    ///
    /// Resolution reads; it never evicts, repairs an anchor, or refreshes
    /// a cache. Those belong to [`Self::evict_lost_anchors`], which runs
    /// while damage can still be staged — a mutation here would land
    /// after the ledger was drained and reach no frame.
    pub fn project(&self, line_of: impl Fn(LineId) -> Option<GridLine>) -> Vec<AnchoredPlacement> {
        self.placements
            .iter()
            .filter_map(|p| {
                Some(AnchoredPlacement {
                    id: p.id,
                    point: GridPoint {
                        line: line_of(p.anchor)?,
                        column: p.col,
                    },
                    size: p.size,
                })
            })
            .collect()
    }

    /// Drops the placements `line_of` can no longer resolve and names
    /// them.
    ///
    /// # Invariants
    ///
    /// `line_of` is the same resolver [`Self::project`] takes. A
    /// placement this rejects is exactly a placement projection would
    /// omit, so no placement can become unresolvable without also
    /// becoming evictable.
    pub fn evict_lost_anchors(
        &mut self,
        line_of: impl Fn(LineId) -> Option<GridLine>,
    ) -> Vec<PlacementId> {
        if self.is_empty() {
            return Vec::new();
        }
        self.evict_where(|p| line_of(p.anchor).is_none())
    }

    /// Empties the table and names every id it held.
    pub fn take_all(&mut self) -> Vec<PlacementId> {
        self.evict_where(|_| true)
    }

    fn evict_where(&mut self, should_evict: impl FnMut(&mut Placement) -> bool) -> Vec<PlacementId> {
        self.placements
            .extract_if(.., should_evict)
            .map(|p| p.id)
            .collect()
    }
}

/// One mounted webview on this screen.
#[derive(Debug)]
struct Placement {
    id: PlacementId,
    anchor: LineId,
    col: GridColumn,
    size: PlacementSize,
    view_id: String,
    instance_id: Option<String>,
}

impl Placement {
    /// Whether this placement is the one `(view_id, instance_id)`
    /// addresses.
    fn addressed_by(&self, view_id: &str, instance_id: Option<&str>) -> bool {
        self.view_id == view_id && self.instance_id.as_deref() == instance_id
    }
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p orzma_vt screen::placements`
Expected: PASS — 5 件

- [ ] **Step 5: `placement.rs` から移設済みのテストを削る**

`crates/orzma_vt/src/placement.rs` の `mod tests` から、Step 1 が引き継いだ 1 本だけを削除する。

| 削除するテスト | 引き継ぎ先 |
| - | - |
| `unmount_honours_its_three_scopes`（`placement.rs:331`） | Step 1 の `an_unmount_removes_the_placements_its_address_names` —— ただし現行は `(view_id, Some(instance_id))` スコープも見ているので、**Step 1 のテストにそのケースを足してから**削除する |

`take_all` 相当のテストは `placement.rs` に存在しない（`mod tests` の 14 本に該当なし）ので、削除対象はこの 1 本だけである。supersession の `re_mounting_a_live_address_succeeds_at_the_cap`（`:317`）は cap 不変条件を固定しているので Task 4 まで残す。cap・id 単調・射影・sweep・`switch_screen` も Task 4 / Task 5 で移すのでここでは残す。

Step 1 の `an_unmount_removes_the_placements_its_address_names` の末尾に、instance 付きで mount したものを instance 付きで落とす往復を足す。これが現行 `unmount_honours_its_three_scopes` の 3 つ目のスコープにあたる。

```rust
        let mut table = table();
        let grid = grid();
        table.mount(
            PlacementId(3),
            grid.line_id(ScreenLine::TOP),
            GridColumn(0),
            PlacementSize { rows: 2, cols: 4 },
            "chart".to_string(),
            Some("a".to_string()),
        );
        assert!(!table.unmount(Some("chart"), Some("b")));
        assert!(table.unmount(Some("chart"), Some("a")));
        assert!(table.is_empty());
```

Run: `cargo test -p orzma_vt`
Expected: PASS

- [ ] **Step 6: lint、format、コミット**

```bash
cargo clippy -p orzma_vt -p orzma_tty_renderer -p orzma_webview --all-targets --fix --allow-dirty --allow-staged && cargo fmt
git add -A
git commit -m "feat(orzma_vt): add the per-screen placement table"
```

---

## Task 3: `Screen` — 8 番目のフィールドと 14 番目の impl ブロック

**Files:**
- Modify: `crates/orzma_vt/src/screen.rs:63-71`（struct）, 末尾（14 番目の impl ブロック）, `mod tests`（14 番目のテストモジュール）

**Interfaces:**
- Consumes: Task 2 の `ScreenPlacements`

> `Screen::reset` は `Option<DamageSpan>` を返し、`Option` は `#[must_use]` なので裸で呼ぶと `unused_must_use` が出る。かつ **overlay はセルを書かないので、placement だけが乗った画面では `Grid::is_blank()` が true になり `reset()` は `None` を返す**（spec「帰結」節の当のケース）。テストでは戻り値を `assert_eq!(.., None)` で受けること —— 警告が消えるうえ、「damage は無いが placement は落ちる」という設計上の要点がテストに残る。
- Produces:
  - `Screen::mount_placement(&mut self, id: PlacementId, size: PlacementSize, view_id: String, instance_id: Option<String>)`
  - `Screen::supersede_placement(&mut self, view_id: &str, instance_id: Option<&str>)`
  - `Screen::unmount_placement(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool`
  - `Screen::take_placements(&mut self) -> Vec<PlacementId>`
  - `Screen::placement_count(&self) -> usize`
  - `Screen::project_placements(&self) -> Vec<AnchoredPlacement>`
  - `Screen::evict_lost_anchors(&mut self) -> Vec<PlacementId>`

- [ ] **Step 1: 失敗するテストを書く**

`crates/orzma_vt/src/screen.rs` の `mod tests` の末尾に 14 番目のテストモジュールを足す。

```rust
    mod placements {
        use super::*;

        fn mount(screen: &mut Screen, id: u64, view: &str) {
            screen.mount_placement(
                PlacementId(id),
                PlacementSize { rows: 2, cols: 4 },
                view.to_string(),
                None,
            );
        }

        /// Asserts that a mount anchors to the row the write cursor sits
        /// on and to the cursor's column.
        ///
        /// Case: a program prints a header, moves the cursor down two
        /// rows and across three columns, and mounts a webview there.
        #[test]
        fn a_mount_anchors_at_the_write_cursor() {
            let mut screen = screen();
            screen.state.line = ScreenLine(2);
            screen.state.column = GridColumn(3);
            mount(&mut screen, 1, "memo");
            let projected = screen.project_placements();
            assert_eq!(projected.len(), 1);
            assert_eq!(projected[0].point.line, GridLine(2));
            assert_eq!(projected[0].point.column, GridColumn(3));
        }

        /// Asserts that a placement whose anchor row left the ring is
        /// omitted by the projection and named by the sweep, so the two
        /// cannot disagree.
        ///
        /// Case: a webview sits on the last row of the screen and a
        /// full-screen application scrolls backwards until that row is
        /// discarded.
        #[test]
        fn a_placement_the_projection_omits_is_also_swept() {
            let mut screen = screen();
            screen.state.line = ScreenLine(2);
            mount(&mut screen, 1, "memo");
            assert_eq!(screen.project_placements().len(), 1);

            for _ in 0..3 {
                screen.reverse_index();
            }

            assert!(screen.project_placements().is_empty());
            assert_eq!(screen.evict_lost_anchors(), vec![PlacementId(1)]);
            assert_eq!(screen.placement_count(), 0);
        }

        /// Asserts that a reset leaves this screen's placements
        /// unresolvable, so the next sweep names all of them.
        ///
        /// Case: an application sends `RIS` while webviews are mounted on
        /// the screen it resets.
        #[test]
        fn a_reset_leaves_this_screens_placements_unresolvable() {
            let mut screen = screen();
            mount(&mut screen, 1, "memo");
            mount(&mut screen, 2, "chart");
            assert_eq!(screen.reset(), None);
            assert!(screen.project_placements().is_empty());
            assert_eq!(
                screen.evict_lost_anchors(),
                vec![PlacementId(1), PlacementId(2)]
            );
        }

        /// Asserts that a re-mount at the same address supersedes the
        /// live placement without naming the superseded id.
        ///
        /// Case: a program re-renders the same named view, and the host
        /// must keep the entity it already spawned for that view.
        #[test]
        fn a_remount_supersedes_without_naming_the_superseded_id() {
            let mut screen = screen();
            mount(&mut screen, 1, "memo");
            screen.supersede_placement("memo", None);
            mount(&mut screen, 2, "memo");
            assert_eq!(screen.placement_count(), 1);
            assert_eq!(screen.take_placements(), vec![PlacementId(2)]);
        }

        /// Asserts that an unmount addressed to a view removes it and
        /// reports that something went.
        ///
        /// Case: a program tears down one of two mounted views.
        #[test]
        fn an_unmount_removes_the_addressed_placement() {
            let mut screen = screen();
            mount(&mut screen, 1, "memo");
            mount(&mut screen, 2, "chart");
            assert!(screen.unmount_placement(Some("memo"), None));
            assert_eq!(screen.placement_count(), 1);
        }
    }
```

`mod tests` の `use` に `crate::placement::{PlacementId, PlacementSize}` を足す。

- [ ] **Step 2: テストを走らせて失敗を確認する**

Run: `cargo test -p orzma_vt screen::tests::placements`
Expected: FAIL — `no method named mount_placement found for struct Screen`

- [ ] **Step 3: フィールドを足す**

`crates/orzma_vt/src/screen.rs:63-71`。

```rust
pub struct Screen {
    grid: Grid,
    viewport: Viewport,
    state: ScreenState,
    scroll_region: ScrollRegion,
    tabs: TabStops,
    character_set_mapping: CharacterSetMapping,
    checkpoint: Checkpoint,
    placements: ScreenPlacements,
}
```

`Screen::new`（`:96` からの Construction ブロック）に `placements: ScreenPlacements::new(),` を足す。`use` に `use crate::placement::{AnchoredPlacement, PlacementId, PlacementSize};` と `use crate::screen::placements::ScreenPlacements;` を足す。

- [ ] **Step 4: 14 番目の impl ブロックを足す**

`crates/orzma_vt/src/screen.rs` の 13 番目のブロック（`/// Whole-screen state replacement.`、`:756` で閉じる）の後ろに置く。

```rust
/// Webview placements.
///
/// The anchor a mount records is a `LineId` from this screen's own grid,
/// so a placement can only ever be resolved against the grid that minted
/// its anchor.
///
/// Four of these forward to [`ScreenPlacements`] unchanged. They stay
/// rather than exposing the table, so `DeviceState` never holds a
/// `&mut ScreenPlacements` and every mutation of a screen's placements
/// goes through the screen that owns them.
impl Screen {
    /// Registers a mount at the write cursor under an already-minted id.
    pub fn mount_placement(
        &mut self,
        id: PlacementId,
        size: PlacementSize,
        view_id: String,
        instance_id: Option<String>,
    ) {
        let anchor = self.cursor_line_id();
        let col = self.cursor_column();
        self.placements
            .mount(id, anchor, col, size, view_id, instance_id);
    }

    /// Drops the placement a re-mount replaces, without reporting it.
    pub fn supersede_placement(&mut self, view_id: &str, instance_id: Option<&str>) {
        self.placements.supersede(view_id, instance_id);
    }

    /// Removes the placements a client `unmount` addresses; returns
    /// whether anything went.
    pub fn unmount_placement(
        &mut self,
        view_id: Option<&str>,
        instance_id: Option<&str>,
    ) -> bool {
        self.placements.unmount(view_id, instance_id)
    }

    /// Empties this screen's table and names every id it held.
    pub fn take_placements(&mut self) -> Vec<PlacementId> {
        self.placements.take_all()
    }

    /// Number of placements this screen holds.
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// Resolves this screen's placements into grid coordinates.
    ///
    /// # Invariants
    ///
    /// The anchors resolve through the same expression
    /// [`Self::evict_lost_anchors`] passes. A placement this omits is
    /// exactly a placement the sweep evicts, so no placement can become
    /// unresolvable without also becoming evictable.
    pub fn project_placements(&self) -> Vec<AnchoredPlacement> {
        self.placements
            .project(|anchor| self.grid.grid_line(anchor))
    }

    /// Drops the placements whose anchor row left this screen's grid and
    /// names them.
    pub fn evict_lost_anchors(&mut self) -> Vec<PlacementId> {
        self.placements
            .evict_lost_anchors(|anchor| self.grid.grid_line(anchor))
    }
}
```

**フィールド分割（`let Self { placements, grid, .. } = self;`）は不要。** sweep はレシーバに `&mut self.placements` を取りながらクロージャが `self.grid` を共有捕捉するが、RFC 2229 の精密キャプチャで捕捉されるのは `self.grid` という place だけなので disjoint として通る（1.95 / edition 2024 で確認済み）。`&self` レシーバのメソッド（`|a| self.grid_line_of(a)`）を渡す形は `*self` 全体を捕捉して E0502 になるので、`self.grid.grid_line` を直に叩くこと。

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p orzma_vt screen::tests::placements`
Expected: PASS — 5 件

- [ ] **Step 6: `Screen::reset` の doc に暗黙の結合を書く**

`crates/orzma_vt/src/screen.rs:723-745` の `Screen::reset` の doc に段落を足す。placement が落ちることはこのメソッドのコードからは見えず、「`Grid::reset` が id を振り直す」＋「チャンク末尾に sweep が走る」の合成で成立する。

```rust
    /// # Invariants
    ///
    /// The cursor lands at the screen's upper-left corner whatever
    /// origin mode was in force, because the state is replaced wholesale
    /// rather than homed through the origin.
    ///
    /// Every placement on this screen becomes evictable here without
    /// this method touching the table: [`crate::screen::grid::Grid::reset`]
    /// mints fresh row ids without rewinding its counter, so no anchor
    /// taken before the reset can resolve afterwards and the next
    /// [`Self::evict_lost_anchors`] names all of them. A rewrite of
    /// `Grid::reset` that renumbers from zero would silently keep the
    /// placements alive.
```

- [ ] **Step 7: 全テスト、lint、コミット**

```bash
cargo test -p orzma_vt -p orzma_tty_renderer -p orzma_webview
cargo clippy -p orzma_vt -p orzma_tty_renderer -p orzma_webview --all-targets --fix --allow-dirty --allow-staged && cargo fmt
git add -A
git commit -m "feat(orzma_vt): let Screen own its placement table"
```

---

## Task 4: `DeviceState` — 採番・cap・アドレス空間・両スクリーン sweep

**Files:**
- Modify: `crates/orzma_vt/src/device.rs:31-40`（struct）, `:42-`（impl）, `mod tests`

**Interfaces:**
- Consumes: Task 3 の `Screen` の 7 メソッド
- Produces:
  - `DeviceState::mount_placement(&mut self, size: PlacementSize, view_id: String, instance_id: Option<String>) -> Option<PlacementId>`
  - `DeviceState::unmount_placement(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool`
  - `DeviceState::evict_lost_anchors(&mut self) -> Vec<PlacementId>`
  - `DeviceState::switch_screen(&mut self, to: ScreenKind) -> Vec<PlacementId>`
  - `pub const MAX_PLACEMENTS: usize`（`placement.rs` で無修飾 → `pub` へ）
  - `DeviceState::placement_count(&self) -> usize` —— **private**。spec の宣言（private グループ）に従う。`mount_placement` と `device.rs` 自身の `mod tests` からしか呼ばれないので、`pub` にすると item-ordering ルール（private を末尾に）にも反する。他タスクは依存しない

- [ ] **Step 1: 失敗するテストを書く**

`crates/orzma_vt/src/device.rs` の `mod tests` に足す。

```rust
    fn mount(device: &mut DeviceState, view: &str) -> Option<PlacementId> {
        device.mount_placement(
            PlacementSize { rows: 2, cols: 4 },
            view.to_string(),
            None,
        )
    }

    /// Asserts that the cap counts both screens, so a mount is rejected
    /// once the pair holds `MAX_PLACEMENTS` between them.
    ///
    /// Case: a program fills the primary screen with webviews, flips to
    /// the alternate screen, and keeps mounting there.
    #[test]
    fn a_mount_at_the_cap_is_rejected_across_both_screens() {
        let mut device = device();
        for index in 0..6 {
            assert!(mount(&mut device, &format!("p{index}")).is_some());
        }
        device.set_active_screen_for_test(ScreenKind::Alternate);
        for index in 0..6 {
            assert!(mount(&mut device, &format!("a{index}")).is_some());
        }
        assert!(mount(&mut device, "one-too-many").is_none());
    }

    /// Asserts that ids keep moving forward across screens and across
    /// unmounts, so a delayed id-addressed event can never name a
    /// successor.
    ///
    /// Case: a program mounts on one screen, unmounts, flips screens, and
    /// mounts again while the host is still acting on the first id.
    #[test]
    fn placement_ids_are_minted_ascending_across_screens() {
        let mut device = device();
        let first = mount(&mut device, "a").expect("first mount accepted");
        device.unmount_placement(None, None);
        device.set_active_screen_for_test(ScreenKind::Alternate);
        let second = mount(&mut device, "b").expect("second mount accepted");
        assert!(first < second);
    }

    /// Asserts that a re-mount of the same address supersedes across the
    /// screen pair rather than leaving a twin on the other screen.
    ///
    /// Case: a program mounts a named view on the primary screen, flips
    /// to the alternate screen, and re-mounts the same name there.
    #[test]
    fn a_remount_supersedes_across_screens() {
        let mut device = device();
        mount(&mut device, "memo").expect("first mount accepted");
        device.set_active_screen_for_test(ScreenKind::Alternate);
        mount(&mut device, "memo").expect("re-mount accepted");
        assert_eq!(device.placement_count(), 1);
    }

    /// Asserts that a broad unmount reaches both screens rather than
    /// stopping at the first match.
    ///
    /// Case: a program mounted the same view on both screens and exits,
    /// so the host despawns every child at that address in one pass.
    #[test]
    fn a_broad_unmount_reaches_both_screens() {
        let mut device = device();
        mount(&mut device, "memo").expect("primary mount accepted");
        device.set_active_screen_for_test(ScreenKind::Alternate);
        mount(&mut device, "chart").expect("alternate mount accepted");
        assert!(device.unmount_placement(None, None));
        assert_eq!(device.placement_count(), 0);
    }

    /// Asserts that the sweep reaches the inactive screen, so a placement
    /// whose anchor died there is still reclaimed.
    ///
    /// Case: `RIS` resets both screens while a webview is mounted on the
    /// one that is not currently shown.
    #[test]
    fn a_sweep_reaches_the_inactive_screen() {
        let mut device = device();
        device.set_active_screen_for_test(ScreenKind::Alternate);
        let id = mount(&mut device, "memo").expect("alternate mount accepted");
        assert_eq!(device.active_mut().reset(), None);
        device.set_active_screen_for_test(ScreenKind::Primary);
        assert_eq!(device.evict_lost_anchors(), vec![id]);
    }

    /// Asserts that resetting both screens leaves every placement
    /// unresolvable, so one terminal-wide sweep names all of them.
    ///
    /// Case: an application sends `RIS` while webviews are mounted on
    /// both the primary and the alternate screen.
    #[test]
    fn a_screen_reset_leaves_every_placement_unresolvable() {
        let mut device = device();
        let primary = mount(&mut device, "shell").expect("primary mount accepted");
        device.set_active_screen_for_test(ScreenKind::Alternate);
        let alternate = mount(&mut device, "app").expect("alternate mount accepted");
        for kind in [ScreenKind::Primary, ScreenKind::Alternate] {
            device.set_active_screen_for_test(kind);
            assert_eq!(device.active_mut().reset(), None);
        }
        device.set_active_screen_for_test(ScreenKind::Primary);
        assert_eq!(device.evict_lost_anchors(), vec![primary, alternate]);
        assert_eq!(device.placement_count(), 0);
    }

    /// Asserts that a re-mount of a live address is accepted at the cap,
    /// because supersession frees the slot it takes before the check.
    ///
    /// Case: a program holding the terminal's last placement slot
    /// re-renders that same named view.
    #[test]
    fn re_mounting_a_live_address_succeeds_at_the_cap() {
        let mut device = device();
        for index in 0..MAX_PLACEMENTS {
            mount(&mut device, &format!("v{index}")).expect("a mount under the cap is accepted");
        }
        assert!(mount(&mut device, "v0").is_some());
        assert_eq!(device.placement_count(), MAX_PLACEMENTS);
    }

    /// Asserts that a flip back to the primary screen tears down the
    /// placements the abandoned alternate screen owned and leaves the
    /// primary's alone.
    ///
    /// Case: a full-screen application that mounted a webview exits, and
    /// the shell's own webview from before it must survive.
    #[test]
    fn a_flip_to_primary_tears_down_only_the_alternate_placements() {
        let mut device = device();
        let kept = mount(&mut device, "shell").expect("primary mount accepted");
        assert!(device.switch_screen(ScreenKind::Alternate).is_empty());
        let dropped = mount(&mut device, "app").expect("alternate mount accepted");
        assert_eq!(device.switch_screen(ScreenKind::Primary), vec![dropped]);
        assert_eq!(device.placement_count(), 1);
        assert_eq!(device.active_mut().take_placements(), vec![kept]);
    }
```

`mod tests` の `use` に `crate::placement::{MAX_PLACEMENTS, PlacementId, PlacementSize}` を足す。`device()` ヘルパが無ければ `fn device() -> DeviceState { DeviceState::new(GridSize { cols: 8, rows: 3 }, 10) }` を足す。

- [ ] **Step 2: テストを走らせて失敗を確認する**

Run: `cargo test -p orzma_vt device::tests`
Expected: FAIL — `no method named mount_placement found for struct DeviceState`

- [ ] **Step 3: `MAX_PLACEMENTS` を `pub` にする**

`crates/orzma_vt/src/placement.rs` の `const MAX_PLACEMENTS: usize = 12;` を `pub const MAX_PLACEMENTS: usize = 12;` にする。既に doc が付いているのでそのまま。

- [ ] **Step 4: `DeviceState` に採番フィールドと 4 つの操作を足す**

`crates/orzma_vt/src/device.rs` の struct に `next_placement_id: PlacementId,` を足し、`DeviceState::new` で `PlacementId(0)` に初期化する。`use` に `use crate::placement::{MAX_PLACEMENTS, PlacementId, PlacementSize};` を足す。

既存の impl の末尾に新しい impl ブロックを置く。

```rust
/// Webview placements.
///
/// The three terminal-scoped invariants live here because none of them
/// can be satisfied by one screen alone: ids are minted per terminal,
/// the cap counts both screens, and a `(view_id, instance_id)` address
/// is unique across the pair.
impl DeviceState {
    /// Registers a mount at the active screen's cursor and mints its id;
    /// `None` when the cap rejects it.
    ///
    /// # Invariants
    ///
    /// Supersession runs before the cap check: a re-mount frees the slot
    /// it takes, so it must succeed even at the limit.
    pub fn mount_placement(
        &mut self,
        size: PlacementSize,
        view_id: String,
        instance_id: Option<String>,
    ) -> Option<PlacementId> {
        self.supersede_placement(&view_id, instance_id.as_deref());
        if MAX_PLACEMENTS <= self.placement_count() {
            return None;
        }
        let id = self.mint_placement_id();
        self.active_mut()
            .mount_placement(id, size, view_id, instance_id);
        Some(id)
    }

    /// Removes the placements a client `unmount` addresses on either
    /// screen; returns whether anything went.
    ///
    /// # Invariants
    ///
    /// Every screen is visited — the accumulation must not short-circuit.
    /// A broad scope (`view_id` alone, or unmount-all) can match on both
    /// screens, and the host despawns every matching child across the
    /// terminal in one pass, so a VT that stopped at the first match
    /// would keep a placement holding a cap slot whose host child is
    /// already gone.
    pub fn unmount_placement(
        &mut self,
        view_id: Option<&str>,
        instance_id: Option<&str>,
    ) -> bool {
        let primary = self.screens.primary.unmount_placement(view_id, instance_id);
        let alternate = self.screens.alternate.unmount_placement(view_id, instance_id);
        primary || alternate
    }

    /// Sweeps both screens for placements whose anchor no longer resolves
    /// and names them.
    ///
    /// # Invariants
    ///
    /// The primary screen's ids come first. The order is observable —
    /// the host acts on the returned list in sequence — and this is the
    /// only operation that exposes it, so it is fixed here.
    pub fn evict_lost_anchors(&mut self) -> Vec<PlacementId> {
        let mut evicted = self.screens.primary.evict_lost_anchors();
        evicted.extend(self.screens.alternate.evict_lost_anchors());
        evicted
    }

    /// Applies an alternate-screen flip, tearing down the placements the
    /// abandoned alternate screen owned.
    ///
    /// Primary placements are hidden while the alternate screen is shown,
    /// not destroyed. This operation stages no damage of its own: the
    /// flip itself must stage `DamageSpan::Full`, which carries the
    /// changed list.
    pub fn switch_screen(&mut self, to: ScreenKind) -> Vec<PlacementId> {
        self.modes.active_screen = to;
        match to {
            ScreenKind::Alternate => Vec::new(),
            ScreenKind::Primary => self.screens.alternate.take_placements(),
        }
    }

    /// Drops the placement a re-mount replaces on either screen.
    fn supersede_placement(&mut self, view_id: &str, instance_id: Option<&str>) {
        self.screens.primary.supersede_placement(view_id, instance_id);
        self.screens.alternate.supersede_placement(view_id, instance_id);
    }

    /// Live placements across both screens — what the cap counts.
    fn placement_count(&self) -> usize {
        self.screens.primary.placement_count() + self.screens.alternate.placement_count()
    }

    /// Hands out the next unused placement id.
    ///
    /// # Invariants
    ///
    /// Ids only ever move forward, which is what lets a delayed
    /// id-addressed lifecycle event be matched against a placement that
    /// may already be gone; the overflow guard is what keeps that true.
    fn mint_placement_id(&mut self) -> PlacementId {
        let id = self.next_placement_id;
        self.next_placement_id = PlacementId(
            id.0.checked_add(1)
                .expect("a session cannot mint u64::MAX placements"),
        );
        id
    }
}
```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p orzma_vt device::tests`
Expected: PASS。Step 1 が足すのは 8 本だが、`device::tests` には既存の `the_two_screens_carry_independent_tab_stops` など（`device.rs:211-238`）があるので、フィルタが拾う総数はそれより多い

- [ ] **Step 6: `DeviceState` の doc を直す**

`crates/orzma_vt/src/device.rs:25-30` の doc は "It owns no parser, placement-extension, damage, or emission state" と言っており、この時点で偽になる。

```rust
/// The emulated terminal device: screens, modes, tabs, colors, title,
/// and the terminal-scoped placement invariants.
///
/// It owns no parser, damage, or emission state — those are the VT's own
/// machinery and sit beside it in [`crate::OrzmaVt`]. The placement
/// table itself belongs to each [`Screen`]; what lives here is only what
/// one screen cannot decide alone: the id counter, the cap across the
/// pair, and the `(view_id, instance_id)` address space.
```

- [ ] **Step 7: 全テスト、lint、コミット**

```bash
cargo test -p orzma_vt -p orzma_tty_renderer -p orzma_webview
cargo clippy -p orzma_vt -p orzma_tty_renderer -p orzma_webview --all-targets --fix --allow-dirty --allow-staged && cargo fmt
git add -A
git commit -m "feat(orzma_vt): move the terminal-scoped placement invariants to DeviceState"
```

---

## Task 5: 読み手の切り替えと旧実装の削除

唯一の破壊的ステップ。`PlacementStore` を引数・フィールドとして持つコードが production とテストの両方にあり、**全て同時にしか切り替えられない**。中断せず完走すること。

**Files:**
- Modify: `crates/orzma_vt/src/frame.rs:139`, `:143`, `:186-193`, `:236-306`, `:391`, `:458-462`
- Modify: `crates/orzma_vt/src/interpreter.rs:42-60`, `:85-105`, `:285-306`
- Modify: `crates/orzma_vt/src/lib.rs:7-16`, `:224`, `:245`, `:264`, `:354-378`
- Modify: `crates/orzma_vt/src/placement.rs`（`PlacementStore` / `Placement` / `mod tests` の削除）
- Modify: `crates/orzma_vt/src/device.rs:79-86`（`active_screen()`）, `:149-181`（`ActiveScreen`）
- Modify: `crates/orzma_vt/src/screen.rs`（`grid_line_of` の削除）

**Interfaces:**
- Consumes: Task 3 / Task 4 の全メソッド
- Produces: `FrameTracker::emit(&mut self, device: &DeviceState) -> Option<Frame>`（第 2 引数が消える）

- [ ] **Step 1: `FrameTracker` を切り替える**

`crates/orzma_vt/src/frame.rs:139`。

```rust
    pub fn emit(&mut self, device: &DeviceState) -> Option<Frame> {
        let screen = device.active();
        let cursor = screen.cursor();
        let display_offset = screen.display_offset();
        let placements = self.diff_placements(device);
```

`:186-193` の `diff_placements`。

```rust
    /// Resolves the active screen's placements and reports the complete
    /// new list when it differs from the last-emitted one; `None` when
    /// unchanged.
    fn diff_placements(&self, device: &DeviceState) -> Option<Vec<AnchoredPlacement>> {
        let projected = device.active().project_placements();
        (projected != self.placements).then_some(projected)
    }
```

`:15` の import から `ActiveScreen` を外す: `use crate::device::DeviceState;`

- [ ] **Step 2: `frame.rs` のテスト `Rig` を切り替える**

`:237-257` の `Rig` から `placements: PlacementStore` フィールドを落とし、`emit` ヘルパを `rig.tracker.emit(&rig.device)` にする。`diff_placements_reports_the_change_until_settled`（`:281-`）と `:391`, `:458-462` の `PlacementStore` を直接駆動する 3 テストを `DeviceState::mount_placement` 経由に書き換える。

```rust
    struct Rig {
        tracker: FrameTracker,
        device: DeviceState,
    }

    fn drained_rig() -> Rig {
        let mut rig = Rig {
            tracker: FrameTracker::new(),
            device: DeviceState::new(GridSize { cols: 4, rows: 3 }, 10),
        };
        emit(&mut rig).expect("the seeded Full drains as the first frame");
        rig
    }

    fn emit(rig: &mut Rig) -> Option<Frame> {
        rig.tracker.emit(&rig.device)
    }
```

`the_first_frame_covers_every_row_and_omits_default_sections`（`frame.rs:389-394`）の `Rig` リテラルからもフィールドを落とす。

```rust
        let mut rig = Rig {
            tracker: FrameTracker::new(),
            device: DeviceState::new(GridSize { cols: 4, rows: 3 }, 10),
        };
```

`a_placement_change_alone_emits_the_complete_list`（`frame.rs:458-476`）は `rig.placements` を直接叩いているので、`DeviceState` 経由に書き換える。

```rust
    #[test]
    fn a_placement_change_alone_emits_the_complete_list() {
        let mut rig = drained_rig();
        rig.device
            .mount_placement(PlacementSize { rows: 2, cols: 4 }, "v".to_string(), None)
            .expect("a mount under the cap is accepted");
        let mounted = emit(&mut rig).expect("a placement change emits");
        assert_eq!(mounted.placements.as_ref().map(Vec::len), Some(1));
        assert!(mounted.rows.is_empty());
        assert_eq!(emit(&mut rig), None);
        assert!(rig.device.unmount_placement(Some("v"), None));
        let unmounted = emit(&mut rig).expect("an unmount emits");
        assert_eq!(unmounted.placements, Some(Vec::new()));
    }
```

```rust
    #[test]
    fn diff_placements_reports_the_change_until_settled() {
        let mut tracker = FrameTracker::new();
        let mut device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        assert_eq!(tracker.diff_placements(&device), None);
        device
            .mount_placement(
                PlacementSize { rows: 2, cols: 4 },
                "v".to_string(),
                None,
            )
            .expect("a mount under the cap is accepted");
        let listed = tracker
            .diff_placements(&device)
            .expect("a mount changes the projection");
        assert_eq!(listed.len(), 1);
        tracker.settle(
            device.active().cursor(),
            device.display_offset(),
            Some(&listed),
            None,
        );
        assert_eq!(tracker.diff_placements(&device), None);
    }
```

- [ ] **Step 3: `Executor` の借用フィールドを落とす**

`crates/orzma_vt/src/interpreter.rs:42-60` の `Interpreter::parse` から `placements: &mut PlacementStore,` 引数を削除し、`Executor` の構築からも外す。`:85-105` の struct から `placements: &'a mut PlacementStore,` を削除。`:285-306` のテストヘルパ `interpret_with` からも外す。`:20-25` の import から `PlacementStore` を外す。

`:92-97` の TODO コメントは残すが、`PlacementStore` への言及を現在の型名に直す。

```rust
    // TODO: The placement and palette handlers (the APC webview verbs
    // and OSC 4 / 10 / 11 / 12) must set this flag when they mutate a
    // frame-visible section — the placement docs promise that "the
    // caller raises the chunk liveness flag", and until those handlers
    // land no caller does.
```

- [ ] **Step 4: `OrzmaVt` からフィールドを落とす**

`crates/orzma_vt/src/lib.rs:224` の `placements: PlacementStore,` フィールド、`:245` の初期化、`:264` の `self.tracker.emit(&self.device, &self.placements)` → `self.tracker.emit(&self.device)`。`:7-16` の import から `PlacementStore` を外し、`placement::PlacementId` だけ残す。

`:354-378` の `a_screen_flip_replays_the_placement_list` を書き換える。現行は `vt.placements.mount(vt.device.active_screen(), ..)` を呼んでいる。

```rust
        vt.device
            .mount_placement(PlacementSize { rows: 2, cols: 4 }, "memo".to_string(), None)
            .expect("a mount under the cap is accepted");
```

`set_active_screen_for_test` を使っている箇所（`:368`, `:374`）はそのまま。

- [ ] **Step 5: `placement.rs` を語彙だけに削ぐ**

`crates/orzma_vt/src/placement.rs` から `PlacementStore` struct と impl、`Placement` struct と impl、`mod tests` を全て削除する。残るのは module doc、`PlacementId`、`AnchoredPlacement`、`PlacementSize`、`MAX_PLACEMENTS` の 5 つ。module doc を書き換える。

```rust
//! Webview placement vocabulary: the id a mount is addressed by, the
//! rectangle it reserves, the grid-space geometry an emitted frame
//! carries, and the per-terminal cap.
//!
//! The table itself belongs to each `Screen`; see
//! [`crate::screen::placements`].
```

`use crate::device::ActiveScreen;`, `use crate::device::modes::ScreenKind;`, `use crate::screen::grid::LineId;` を削除し、`use crate::screen::grid::coords::GridPoint;` だけ残す。

**`PlacementStore::reset()`（`placement.rs:199-200`）は呼び出し元ゼロの空実装。** 型ごと消えるので、`Screen` にも `DeviceState` にも移植しない。RIS の破棄は `Grid::reset` の id 振り直し ＋ チャンク末尾の両スクリーン sweep の合成で成立する（Task 3 Step 6 の doc を参照）。

削除される回帰テスト `a_sweep_leaves_the_other_screens_placement_alone` は、守る失敗モード（非アクティブ側の anchor をアクティブな grid で解決する経路）が型として存在しなくなるため復活させない。

**`mod tests` の残りは移設先を持つ。** そのまま消すと 2 つの不変条件が無防備になる。

| 消えるテスト | 移設先 |
| - | - |
| `re_mounting_a_live_address_succeeds_at_the_cap`（`:317`） | Task 4 Step 1 の同名テストが引き継ぎ済み |
| `an_anchor_still_resolves_once_the_history_ids_are_unordered`（`:537`） | **`screen/grid.rs` の `mod tests` へ下ろす**（下記）。`Grid::grid_line` が `position` ではなく `rposition` である理由を守る唯一のテストで、`screen/grid.rs` には同等のものが無い |
| `ids_are_minted_ascending_and_never_repeat`、`a_mount_past_the_cap_is_rejected`、`an_unmount_matching_nothing_reports_no_change`、射影・sweep・`switch_screen` の各テスト | Task 2 / Task 3 / Task 4 の同等テストが既に引き継いでいる |

`crates/orzma_vt/src/screen/grid.rs` の `mod tests` に足す。`LineId` の doc（`grid.rs:40-48`）が「リングは id で順序づかない」と宣言している当のケースである。

```rust
        /// Asserts that an anchor still resolves once the history holds
        /// ids that are no longer ascending, which is why the lookup
        /// scans rather than binary-searches.
        ///
        /// Case: a full-screen application scrolls backwards — minting a
        /// row with a high id above older rows — and then output pushes
        /// that row into history ahead of the ones it was inserted above.
        #[test]
        fn an_anchor_still_resolves_once_the_history_ids_are_unordered() {
            let mut grid = grid(3, 10);
            let anchor = grid.line_id(ScreenLine::TOP);
            grid.scroll_down_one(ScreenLine(0), ScreenLine(2), Cell::default());
            for _ in 0..3 {
                scroll_up_whole_screen(&mut grid, Cell::default());
            }
            assert_eq!(grid.grid_line(anchor), Some(GridLine(-3)));
        }
```

期待値は転記時に実測で確かめること —— 元のテストは `Screen` 経由（`reverse_index` + `line_feed` × 3）で viewport 行 0 を主張していた。`Grid` 直叩きに下ろすと行番号の基準が変わるので、`grid_line` が `Some(..)` を返すこと（＝リニアスキャンが非単調な履歴でも当てること）が守るべき本体であり、具体的な `GridLine` の値はその副産物である。

- [ ] **Step 6: `ActiveScreen` と `grid_line_of` を削除する**

- `crates/orzma_vt/src/device.rs:79-86` の `DeviceState::active_screen()` を削除
- `crates/orzma_vt/src/device.rs:149-181` の `ActiveScreen` struct と impl を丸ごと削除
- `crates/orzma_vt/src/screen.rs` の `Screen::grid_line_of`（Task 1 Step 4 で足した暫定シム）を削除
- `device.rs` の `use` から、これで未使用になるものを整理する。**`GridColumn` は残す** —— `device.rs` 自身の `mod tests` が `cursor_column()` の主張に使っている（`:228`, `:233`, `:260`, `:265`）ので、消すとテストビルドが壊れる。実際に外れるのは Task 1 で足した `GridLine` と、`ActiveScreen` と共に消える `LineId` の 2 つ

`DeviceState::active()` / `active_mut()` / `set_active_screen_for_test`、`ScreenKind` は残る。

- [ ] **Step 7: 全テストを走らせる**

Run: `cargo test -p orzma_vt -p orzma_tty_renderer -p orzma_webview`
Expected: PASS

- [ ] **Step 8: lint、format、コミット**

```bash
cargo clippy -p orzma_vt -p orzma_tty_renderer -p orzma_webview --all-targets --fix --allow-dirty --allow-staged && cargo fmt
git add -A
git commit -m "refactor(orzma_vt): retire PlacementStore and ActiveScreen"
```

---

## Task 6: 追随が必要な文書

**Files:**
- Modify: `docs/todo/ris.md`
- Modify: `docs/todo/tdd-placement-reset.md`
- Modify: `docs/todo/placement-ownership.md`
- Modify: `crates/orzma_vt/src/device.rs:96-101`（reflow の TODO）

**Interfaces:**
- Consumes: Task 5 完了後のツリー
- Produces: なし（文書のみ）

- [ ] **Step 1: `ris.md` を直す**

- #4（`PlacementStore::clear()`）の行を削除する。`Grid::reset` が id を振り直し、チャンク末尾の両スクリーン sweep が全 placement を回収するので、全消し専用 API は要らない
- #5 の RIS ハンドラの記述から placement 専用の liveness 操作と signal 送出を落とし、「`device.reset()` を呼んで damage を stage する」だけにする
- 決定事項 2 の記述を「sweep 経由で破棄」に直す

- [ ] **Step 2: `tdd-placement-reset.md` の 7 ケースを直す**

| ケース | 書き換え |
| - | - |
| TC-01 全破棄して id を名指す | reset → `evict_lost_anchors()` の 2 段で検証する形に |
| TC-02 viewport 外だが history に在る anchor | 残す。reset 前は sweep が残し reset 後は落とす、を対比で固定。**禁止の理由が変わったことを明記する** —— 元の TC-02 は「reset が sweep の述語を流用する実装」を禁じるために書かれたが、本設計はまさにそれを採る。成立するのは `Grid::reset` が先に走って anchor を破壊するからで、その順序が根拠であることを残さないと将来の読み手が旧い禁止を再導出する |
| TC-03 非アクティブスクリーン | `a_sweep_reaches_the_inactive_screen`（Task 4）に統合済みと記す |
| TC-04 既に grid から消えた anchor | reset 前から `None` なので reset 有無に関わらず落ちる |
| TC-05 2 度目の reset は空 | そのまま |
| TC-06 cap 解放 | 「sweep の後に mount が受理される」に書き換え |
| TC-A1 id counter を巻き戻さない | 対象を `DeviceState::next_placement_id` に変更 |

- [ ] **Step 3: `placement-ownership.md` を検討メモとして畳む**

内容は spec が引き継いだので、検討メモ側には「採用が決まった」旨と `docs/superpowers/specs/2026-08-26-placement-ownership-design.md` へのリンクだけを残す。

- [ ] **Step 4: `device.rs` の reflow TODO を直す**

`crates/orzma_vt/src/device.rs:96-101` の TODO は「placement が兄弟フィールドなので caller が行リマップを配送する必要がある」と書いている。各 `Screen` が自分の表を持つ形になったので、その前提が消えた。

```rust
    // TODO: Reflow must remap each screen's placement anchors as it
    // rewraps that screen's rows. The table now lives on the screen
    // whose grid minted its anchors, so the remap happens inside
    // `Screen::resize` rather than being delivered by this caller.
```

- [ ] **Step 5: 文書だけのコミット**

```bash
git add -A
git commit -m "docs: follow the placement-ownership move through the todo notes"
```

---

## Self-Review

**1. Spec coverage**

| spec のセクション | 実装するタスク |
| - | - |
| `placement.rs` — 公開語彙と cap だけが残る | Task 1 Step 3、Task 5 Step 5 |
| 座標空間 — placement は grid 空間で出る | Task 1 全体 |
| 可視性の書き方 | Global Constraints、各 Task のコード |
| `screen/placements.rs` — 1 スクリーン分の表 | Task 2 |
| `Screen` — 14 番目の impl ブロック | Task 3 |
| `DeviceState` — 端末スコープの 3 つ | Task 4 |
| `FrameTracker` と `Executor` | Task 5 Step 1-4 |
| eviction 経路の一本化 / 両スクリーン sweep | Task 4 Step 4（`evict_lost_anchors`）、Task 4 Step 1（`a_sweep_reaches_the_inactive_screen`） |
| 帰結 — `Screen::reset` は placement について何も返さない | Task 3 Step 1（`a_reset_leaves_this_screens_placements_unresolvable`）・Step 6（doc）、Task 4 Step 1（両スクリーン版の T-N1） |
| 消えるもの（11 項目） | Task 1 Step 4-5、Task 5 Step 3-6 |
| テスト計画（移設 / 削除 / T-N1〜T-N6） | Task 1 Step 1・5、Task 2 Step 1・5、Task 3 Step 1、Task 4 Step 1、Task 5 Step 2・5 |
| `tdd-placement-reset.md` の 7 ケース | Task 6 Step 2 |
| 実装上の注意（コメント分類、`extract_if`） | Global Constraints、Task 2 Step 3 |
| 移行手順 6 段階 | Task 1〜6 に 1 対 1 |
| 追随が必要な文書 | Task 6 |

T-N1〜T-N6 の対応:

| spec の ID | 実装場所 |
| - | - |
| T-N1 `a_screen_reset_leaves_every_placement_unresolvable` | Task 4 Step 1 —— spec が指定するとおり `DeviceState` スコープで、両スクリーンに mount → 各 `Screen::reset` → 1 回の sweep が全 id を返す。Task 3 Step 1 の `a_reset_leaves_this_screens_placements_unresolvable` は同じ連鎖を 1 スクリーンで先に固定する下敷き |
| T-N2 `a_sweep_reaches_the_inactive_screen` | Task 4 Step 1 |
| T-N3 `a_mount_at_the_cap_is_rejected_across_both_screens` | Task 4 Step 1 |
| T-N4 `a_remount_supersedes_across_screens` | Task 4 Step 1 |
| T-N5 `a_placement_the_projection_omits_is_also_swept` | Task 3 Step 1 |
| T-N6 `a_broad_unmount_reaches_both_screens` | Task 4 Step 1 |

**2. 未着手として明示しておく前提**

spec の「本設計が前提にしている、まだ結線されていないもの」はこの計画のスコープ外である。実装後も次の 3 つは `todo!()` のまま残る。

- `Executor` がチャンク末尾に `DeviceState::evict_lost_anchors` を呼ぶ（`Vt::interpret` が `todo!()`、`lib.rs:260`）
- `Executor` が evicted id を `VtSignal::WebviewEvicted` に載せる（signal の outbox が TODO、`interpreter.rs:91`）
- sweep の結果が非空なら chunk liveness を立てる（同上）

この計画が確定させるのは、その段が呼ぶ API の形までである。

**ワイヤ表現のギャップ（spec も本計画も解いていない）。** `Frame.placements` は `Option<Vec<AnchoredPlacement>>` で `None` が「前フレームから不変」を意味するが、`FrameSnapshot.placements` / `FrameDelta.placements`（`crates/orzma_tty_renderer/src/schema/frame.rs:31`, `:60`）は素の `Vec` で、この 3 状態を表現できない。spec が挙げる副次的利得 —— 「ユーザースクロールでは `placements: None` に落ちて再送されない」 —— は、ワイヤ変換が `None` を `vec![]` に潰さないことが前提であり、潰すとレンダラは宣言的な全置換を行うのでスクロールのたびに全オーバーレイが消える。

現時点では潜在的な問題にとどまる。`orzma_tty_engine` の `build_snapshot` / `build_delta` はまだ `placements` を一切埋めていない。結線する側が `Option` を素通しするか、変換時に「不変ならそのフレーム自体を出さない」を守る必要がある。本計画では**解かず、ここに記録するにとどめる**。

**ワークスペース全体はもともと通らない。** `orzma_tty_engine` が `FrameSnapshot` / `ViCursor` / `SelectionKind` のフィールド不一致で 32 件のエラーを抱えている。本計画とは無関係の既存の壊れで、検証コマンドを 3 クレート指定にしているのはこのためである。着手前の状態に戻す責任は本計画にはない。

**3. 型の一貫性**

- `AnchoredPlacement` の綴りは Task 1 で導入し、Task 2 / 3 / 5 で同じ綴りを使っている
- `ScreenPlacements::evict_lost_anchors` と `Screen::evict_lost_anchors` と `DeviceState::evict_lost_anchors` は同名の 3 層で、引数が `impl Fn(LineId) -> Option<GridLine>` / なし / なし と異なる。呼び違えはコンパイルエラーになる
- `Screen::grid_line_of` は Task 1 で導入し Task 5 Step 6 で削除する暫定シムであることを両方に明記した
- `PlacementStore::project` は Task 1 で `Vec<AnchoredPlacement>` を返すよう変わり、Task 5 で型ごと消える
- `a_screen_reset_leaves_every_placement_unresolvable`（Task 4、`DeviceState` スコープ）と `a_reset_leaves_this_screens_placements_unresolvable`（Task 3、`Screen` スコープ）は別名にしてある。同名にすると spec の T-N1 がどちらを指すのか読めなくなる
- `DeviceState::placement_count` は private。他タスクは依存せず、Task 4 の `mount_placement` と同ファイルの `mod tests` だけが呼ぶ
