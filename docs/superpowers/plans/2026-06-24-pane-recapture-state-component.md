# PaneRecaptureState コンポーネント化 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `recapture_settled_panes` の per-pane 状態を `Local<HashMap>` から `TmuxPane` エンティティ上の `PaneRecaptureState` コンポーネント(`#[require]`)へ移し、`watch`/`last_gateway` の両 Local と F3 ワークアラウンドを撤去する(振る舞い不変)。

**Architecture:** `PaneRecaptureState` を `components.rs` で `#[derive(Component, Default)]` 化し `TmuxPane` に `#[require(PaneRecaptureState)]` を付与。`recapture_settled_panes` は `Query<(&TmuxPane, &mut PaneRecaptureState)>` を回す。pane エンティティは `ChildOf(window)` でリコネクト/セッション切替/ウィンドウクローズ時に cascade despawn されるため、リコネクト時の再シードは pane の despawn→respawn で構造的に成立し、F3 の `last_gateway`+`watch.clear()` が不要になる。

**Tech Stack:** Rust 2024 / toolchain 1.95、Bevy 0.18 ECS(required components `#[require]`、`Single`、`Query`)。

**Spec:** `docs/superpowers/specs/2026-06-24-pane-recapture-state-component-design.md`(spec-review 反映済み、両レビュアー High/code-verified)。

## Global Constraints

- 英語コメントのみ。非doc行コメントは `// TODO:`/`// NOTE:`/`// SAFETY:` のみ(`// NOTE:` は重大 caveat 限定)。
- `mod.rs` 禁止。`pub` は外部API のみ。モジュール外に呼び出しが無い項目は private/`pub(crate)`。
- すべての `use` はファイル先頭の連続ブロック。インライン完全修飾パス禁止。
- `Query`/`Single` パラメータに `_q` サフィックス禁止。可変パラメータを不変より前に。
- `impl`/モジュール内は可視性降順。
- change-detection は条件付き書き込みで駆動(`set_changed`/`bypass_change_detection` 不使用)。
- 変更後 `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`。
- 末尾で `cargo build`(ワークスペース)と `cargo test -p ozmux_tmux` が緑。

---

## File Structure

- `crates/tmux_session/src/components.rs` — `PaneRecaptureState`(新規 component)を `TmuxPane` の隣に定義し、`TmuxPane` に `#[require(PaneRecaptureState)]` を付与。
- `crates/tmux_session/src/plugin.rs` — 旧 `PaneRecaptureState` struct を削除、`recapture_settled_panes` を component クエリへ書き換え、`use std::collections::{HashMap, HashSet};` を削除、`use crate::components::{...}` に `PaneRecaptureState` を追加、F3 テストの名前/コメントを更新。`RECAPTURE_SETTLE_FRAMES` 定数とシステム登録(`build()`)は据え置き。

この変更は **原子的**(`PaneRecaptureState` を二箇所に同名で置けず、システムも半端には移行できない)なので 1 タスク・複数ステップとする。既存の2テスト(`recapture_rearms_after_pane_size_change`、F3 テスト)が安全網。

---

## Task 1: PaneRecaptureState をコンポーネント化し watch/last_gateway を撤去

**Files:**
- Modify: `crates/tmux_session/src/components.rs`(`PaneRecaptureState` 定義追加、`TmuxPane` に `#[require]`)
- Modify: `crates/tmux_session/src/plugin.rs`(struct 削除、system 書き換え、import 調整、F3 テスト更新)

**Interfaces:**
- Consumes: `TmuxPane`(`components.rs`、`{ id: PaneId, dims: CellDims }`)、`TmuxClient`/`EnumerationState`(同一 gateway エンティティ上)、`request_pane_capture(client: &mut TmuxClient, enumeration: &mut EnumerationState, pane: PaneId)`、`RECAPTURE_SETTLE_FRAMES: u8`。
- Produces: `pub(crate) struct PaneRecaptureState`(`#[derive(Component, Default)]`、フィールド private)を `components.rs` に。`recapture_settled_panes(mut client: Single<(&mut TmuxClient, &mut EnumerationState)>, mut panes: Query<(&TmuxPane, &mut PaneRecaptureState)>)`。

- [ ] **Step 1: `components.rs` に `PaneRecaptureState` を追加し `TmuxPane` に `#[require]`**

`crates/tmux_session/src/components.rs` の `TmuxPane` の定義(`#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]` + `pub struct TmuxPane { ... }`)を以下に置換:

```rust
/// A projected tmux pane entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(PaneRecaptureState)]
pub struct TmuxPane {
    /// tmux pane id (`%N`).
    pub id: PaneId,
    /// Cell geometry from the window layout.
    pub dims: CellDims,
}

/// Per-pane bookkeeping for the recapture-settle re-seed (driven by
/// `recapture_settled_panes` in the plugin). Co-located with `TmuxPane` so it
/// can be a required component; the re-seed logic itself lives in the plugin.
/// Despawning the pane drops this, so a reconnect that respawns the pane starts
/// from a clean re-seed state.
#[derive(Component, Default)]
pub(crate) struct PaneRecaptureState {
    /// Last-seen cell dims, to detect size changes.
    dims: (u32, u32),
    /// Frames the dims have held steady since the last change.
    stable: u8,
    /// Whether this pane has been re-seeded since its last size change.
    done: bool,
}
```

NOTE: `#[require(...)]` は `Component` derive のヘルパ属性なので追加の `use` は不要(`use bevy::prelude::Component;` で足りる)。フィールドは private、型は `pub(crate)`(components.rs の `#[require]` と plugin.rs のクエリが crate 内で参照するため)。

- [ ] **Step 2: `plugin.rs` の旧 `PaneRecaptureState` struct を削除**

`crates/tmux_session/src/plugin.rs` の以下(`PaneRecaptureState` 定義、`recapture_settled_panes` の doc コメント直前にある)を**丸ごと削除**:

```rust
/// Per-pane state for [`recapture_settled_panes`].
#[derive(Default)]
struct PaneRecaptureState {
    /// Last-seen cell dims, to detect size changes.
    dims: (u32, u32),
    /// Frames the dims have held steady since the last change.
    stable: u8,
    /// Whether this pane has been re-seeded since its last size change.
    done: bool,
}
```

- [ ] **Step 3: `plugin.rs` の import を調整**

`crates/tmux_session/src/plugin.rs` 先頭の `use crate::components::{TmuxPane, TmuxSession};` を:

```rust
use crate::components::{PaneRecaptureState, TmuxPane, TmuxSession};
```

そして `use std::collections::{HashMap, HashSet};` の行を**削除**(削除後、plugin.rs に `HashMap`/`HashSet` の利用が残らないことを `grep -n "HashMap\|HashSet" crates/tmux_session/src/plugin.rs` で確認 — 一致はコメント行のみのはず)。

- [ ] **Step 4: `recapture_settled_panes` を書き換え**

`crates/tmux_session/src/plugin.rs` の `fn recapture_settled_panes(...) { ... }`(`Local<HashMap<...>> watch` / `Local<Option<Entity>> last_gateway` / `present`/`retain`/gateway-change clear を含む現行ボディ全体)を以下に置換。doc コメント(`/// Re-seeds each pane's ...` から始まる長い NOTE)はそのまま残す:

```rust
fn recapture_settled_panes(
    mut client: Single<(&mut TmuxClient, &mut EnumerationState)>,
    mut panes: Query<(&TmuxPane, &mut PaneRecaptureState)>,
) {
    let (client, enumeration) = &mut *client;
    for (pane, mut state) in panes.iter_mut() {
        let dims = (pane.dims.width, pane.dims.height);
        if state.dims != dims {
            // NOTE: re-arm the one-shot — a size change (e.g. a born-small
            // adopted pane grown to the client size) pulls scrollback onto the
            // screen and needs a fresh re-seed once the new size settles.
            *state = PaneRecaptureState {
                dims,
                stable: 0,
                done: false,
            };
        } else if !state.done && state.stable < RECAPTURE_SETTLE_FRAMES {
            state.stable += 1;
        }
        if !state.done
            && state.stable >= RECAPTURE_SETTLE_FRAMES
            && !enumeration.panes_with_cursor_pending.contains(&pane.id)
        {
            state.done = true;
            request_pane_capture(client, enumeration, pane.id);
        }
    }
}
```

NOTE: `*state = PaneRecaptureState { .. }` はフィールド private だが `components.rs` で定義した同名フィールドへ同一 crate からアクセスできる(plugin.rs と components.rs は同一 crate)。increment を `state.stable < RECAPTURE_SETTLE_FRAMES` でガードするのは、settle 後に同値を毎フレーム書いて `Changed<PaneRecaptureState>` を空打ちしないため(change-detection ハイジーン)。

- [ ] **Step 5: F3 テストの名前とコメントを更新**

`crates/tmux_session/src/plugin.rs` のテスト `recapture_clears_watch_on_gateway_change_so_reconnect_reseeds_reused_pane_id` を改名し、`Local<HashMap>`/`watch.clear()` を参照する2つのコメントを実機構(pane エンティティ despawn)に更新。**テスト本体(spawn/despawn/assert)は変更不要**(既に old pane を despawn し reused-id の fresh pane を spawn している)。

関数シグネチャ行:

```rust
    fn recapture_reseeds_reused_pane_id_after_pane_respawn() {
```

冒頭の Regression コメント(`// Regression: after teardown + re-adoption, recapture_settled_panes kept` から始まる4行)を:

```rust
        // Regression: a reconnect to a restarted tmux server can reuse a pane id.
        // PaneRecaptureState now lives on the pane entity, so despawning the old
        // pane drops its `done: true` state and the respawned reused-id pane gets
        // a fresh component — the one-shot re-seed must fire again on reconnect.
```

末尾近くの `// Settle the reconnected pane — the watch.clear() on gateway change must` の2行コメントを:

```rust
        // Settle the reconnected pane — the fresh PaneRecaptureState on the
        // respawned pane entity must re-arm the one-shot so the re-seed fires.
```

- [ ] **Step 6: ビルドとテストの確認**

Run: `cargo build 2>&1 | tail -5`
Expected: 成功(ワークスペース、binary 含む)。

Run: `cargo test -p ozmux_tmux 2>&1 | tail -20`
Expected: 全テスト PASS。特に `recapture_rearms_after_pane_size_change`(変更不要 — `spawn(TmuxPane{..})` が `#[require]` で `PaneRecaptureState` を自動付与)と改名後の `recapture_reseeds_reused_pane_id_after_pane_respawn` が緑。

Run: `grep -rn "watch\|last_gateway\|Local<HashMap" crates/tmux_session/src/plugin.rs`
Expected: recapture 由来の参照なし(他システムの無関係な一致が無いことを確認)。

- [ ] **Step 7: lint & commit**

```bash
cargo clippy --workspace --all-targets 2>&1 | tail -5
cargo fmt
git add -A
git commit -m "refactor(tmux): move PaneRecaptureState onto the pane entity; drop recapture Locals"
```
Expected: clippy 警告なし、fmt 差分なし、コミット成功。

---

## Self-Review メモ

- **Spec coverage**: コンポーネント化+`#[require]`(Step 1)、両 Local/`retain`/F3 clear 撤去(Step 4)、import 削除(Step 3)、system 書き換え(Step 4)、テスト更新(Step 5)、change-detection ガード(Step 4)— spec の全項目に対応。
- **Placeholder**: なし(全ステップに実コード/実コマンド)。
- **型整合**: `PaneRecaptureState`(components.rs, `pub(crate)`, フィールド `dims:(u32,u32)`/`stable:u8`/`done:bool`)を plugin.rs が `use` し `*state = PaneRecaptureState{..}` で構築 — 一致。`recapture_settled_panes` の新シグネチャは登録行(`build()` の `(request_pane_captures, recapture_settled_panes)`)を変えない(Bevy がパラメータ解決)。
- **原子性**: 1 タスクで crate 内完結。中間状態は作らない(struct 移動 + system 書き換えを同一コミットで)。
