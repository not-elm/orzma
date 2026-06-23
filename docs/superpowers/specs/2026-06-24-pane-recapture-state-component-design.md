# `PaneRecaptureState` を pane コンポーネント化し `watch` Local を撤去

- **日付**: 2026-06-24
- **対象**: `crates/tmux_session/src/{plugin.rs,components.rs}`
- **ステータス**: 設計承認済み(実装プラン未作成)
- **前提**: `docs/superpowers/specs/2026-06-23-tmux-connection-redesign-design.md`(TmuxClient コンポーネント化)完了後のコードベース。

## 背景と動機

`recapture_settled_panes`(`crates/tmux_session/src/plugin.rs`)は、各 tmux pane の
ミラーを「サイズが落ち着いた数フレーム後に tmux の権威グリッドから再シードする」
一回限りのロジック。現在の per-pane 状態は **`Local` 変数**に保持されている:

```rust
fn recapture_settled_panes(
    mut watch: Local<HashMap<PaneId, PaneRecaptureState>>,
    mut last_gateway: Local<Option<Entity>>,
    mut client: Single<(Entity, &mut TmuxClient, &mut EnumerationState)>,
    panes: Query<&TmuxPane>,
) { ... }
```

- `watch` は `PaneId → PaneRecaptureState`(`dims`/`stable`/`done`)の手動マップ。
- `last_gateway` は直近の TmuxClient 再設計レビュー(F3 指摘)で追加された **ワーク
  アラウンド**: `Local` がリコネクトをまたいで残るため、ゲートウェイ・エンティティが
  変わったら `watch.clear()` する。リコネクト先の tmux サーバが pane id を再利用すると
  古い `done: true` が残って再シードを握り潰す問題への対処。
- 毎フレーム `watch.retain(|id| present.contains(id))` で離脱した pane を間引く。

`Local` での状態管理は、直近完了した「per-connection 状態をコンポーネント化する」
リファクタの方針と噛み合わない。pane ごとの状態は **pane エンティティ自身**に
コンポーネントとして載せるのが ECS 的で、`watch`・`retain`・F3 ワークアラウンドを
まとめて撤去できる。

### 鍵となる事実(調査で確認済み)

- `TmuxPane` エンティティは `ChildOf(window)`(`observers.rs:287`)。`despawn_window`
  はウィンドウを despawn し、**ChildOf の pane エンティティへ cascade**する
  (`observers.rs:306-308`)。
- `on_connection_reset`(`observers.rs:222`)は全ウィンドウを despawn → 全 pane が
  cascade で despawn。teardown と再 adopt の双方がこれを通る。
- session-switch も `TmuxWindowsRetained{windows: vec![]}` を発火 → 全ウィンドウ
  despawn → pane cascade。
- layout-change での既存 pane は**エンティティを再利用**(`observers.rs:284`,
  `insert((pane, ChildOf(window)))`)し、despawn しない。

帰結: **リコネクト/セッション切替/ウィンドウクローズでは pane エンティティが
despawn→respawn される**ので、pane に載せたコンポーネントは自然に破棄され、
リコネクトで pane id が再利用されても新しい pane エンティティは fresh な状態を持つ。
これは F3 ワークアラウンド(`last_gateway` + `watch.clear()`)が解決していた問題を
**構造的に解消**する。layout-change の再利用 pane ではコンポーネントが残り、
`dims` 比較で正しく再アームされる(現 `watch` がエントリを保持するのと同値)。

## ゴールとスコープ

### ゴール

- `PaneRecaptureState` を `TmuxPane` エンティティ上の **Component** にする。
- `watch: Local<HashMap<…>>` と `last_gateway: Local<Option<Entity>>` の**両 Local を
  撤去**し、`retain` プルーンと F3 の gateway-change clear も削除する。
- **振る舞いは不変**(再シードのタイミング・条件は維持。リコネクト時の再シードは
  pane despawn により成立)。

### 非ゴール

- 再シードのロジック/しきい値(`RECAPTURE_SETTLE_FRAMES`)の変更。
- `TmuxProjection` 索引や他の pane 関連状態の変更。

## 設計

### 1. コンポーネント & 配置

`PaneRecaptureState` を `crates/tmux_session/src/components.rs` の `TmuxPane` の隣に
移し、コンポーネント化する(フィールドは現状のまま):

```rust
/// Per-pane bookkeeping for the recapture-settle re-seed (see
/// `recapture_settled_panes`). Co-located with `TmuxPane` so it can be a
/// required component; the re-seed logic itself lives in the plugin.
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

`TmuxPane`(`components.rs`)に `#[require(PaneRecaptureState)]` を付与:

```rust
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[require(PaneRecaptureState)]
pub struct TmuxPane { /* unchanged */ }
```

- **配置の根拠**: `PaneRecaptureState` は pane エンティティに付く状態なので、pane
  コンポーネント群と同居が自然。`#[require]` がローカル解決でき、components.rs ↔
  plugin.rs の相互(循環)依存を作らない。
- **可視性**: フィールドは private のまま。型は components.rs が `#[require]` で参照し、
  plugin.rs がクエリで参照するため、crate 内可視(`pub(crate)`)で足りる(外部公開
  不要)。`TmuxPane` 自体は既に `pub`。
- `RECAPTURE_SETTLE_FRAMES` 定数と再シードのシステム/ヘルパは `plugin.rs` に据え置く。

### 2. システム書き換え(`recapture_settled_panes`)

```rust
fn recapture_settled_panes(
    mut client: Single<(&mut TmuxClient, &mut EnumerationState)>,
    mut panes: Query<(&TmuxPane, &mut PaneRecaptureState)>,
) {
    let (client, enumeration) = &mut *client;
    for (pane, mut state) in panes.iter_mut() {
        let dims = (pane.dims.width, pane.dims.height);
        if state.dims != dims {
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

**削除されるもの**:
- `watch: Local<HashMap<PaneId, PaneRecaptureState>>` とその `entry().or_insert()`。
- `last_gateway: Local<Option<Entity>>` と gateway-change 時の `watch.clear()`(F3)。
- `present: HashSet<PaneId>` と `watch.retain(...)` プルーン。
- `Single` の先頭 `Entity`(F3 でのみ必要だった)。
- `plugin.rs` の `use std::collections::{HashMap, HashSet};`(他に利用なしを確認済み)。

**change-detection への配慮**: `state.stable` の increment を
`state.stable < RECAPTURE_SETTLE_FRAMES` でガードし、しきい値到達後は書き込まない。
これにより settle 後に同値を毎フレーム書いて `Changed<PaneRecaptureState>` を空打ち
することがない(リポジトリの「変更時のみ書く」方針に沿う)。現状 `Changed` を読む
消費者は無いが、コンポーネント化に伴うハイジーンとして守る。

**タイミングの微差(許容)**: `#[require]` は `PaneRecaptureState::default()`(dims
`(0,0)`)を挿入するため、初回フレームで `(0,0) != 実 dims` の再アームが1回入り、
最初の再シードが現状より最大1フレーム遅れる。`RECAPTURE_SETTLE_FRAMES = 3` の
デバウンス下で無害。

### 3. データフロー

- pane spawn(`observers.rs` の upsert / `ensure_pane`)→ `#[require]` で
  `PaneRecaptureState::default()` が自動付与。
- 同一セッションの layout-change → pane エンティティ再利用 → `PaneRecaptureState`
  保持 → `dims` 比較で再アーム。
- pane close / window close / reconnect / session-switch → pane エンティティ despawn
  → `PaneRecaptureState` も破棄。再 enumerate で fresh pane に fresh state。

## テスト

- **`recapture_clears_watch_on_gateway_change_so_reconnect_reseeds_reused_pane_id`**
  (`plugin.rs`): 既に old pane を despawn し、reused id の fresh pane を spawn して
  再シードを検証している。コンポーネント・モデルでは「despawn で旧 state 破棄 →
  fresh pane に fresh state」で**そのままパス**する。テスト名を
  `recapture_reseeds_reused_pane_id_after_pane_respawn` 等へ改名し、コメントの機構
  説明を `watch.clear()` から「pane エンティティ despawn」に更新。
- **`recapture_rearms_after_pane_size_change`**(`plugin.rs`): `TmuxPane` を spawn
  (`#[require]` で `PaneRecaptureState` 自動付与)→ settle で再シード → `TmuxPane.dims`
  を変更 → settle で再シード。`SETTLE+1` フレーム数は #[require] の初回1フレーム差を
  吸収して足りる。
- いずれのテストも `watch`/`last_gateway`/`HashMap` への直接依存は元々無い
  (EnumerationState の `pending` を見る)ので、システムのパラメータ変更に追従する
  だけ。

## 影響を受けるファイル

- `crates/tmux_session/src/components.rs`: `PaneRecaptureState` 定義を追加、`TmuxPane`
  に `#[require(PaneRecaptureState)]`。
- `crates/tmux_session/src/plugin.rs`: `PaneRecaptureState` 定義を削除、
  `recapture_settled_panes` を書き換え、`use std::collections::{HashMap, HashSet}`
  を削除、テスト2件を更新。`RECAPTURE_SETTLE_FRAMES` は据え置き。

## Global Constraints

- 英語コメントのみ。`// TODO:`/`// NOTE:`/`// SAFETY:` のみ(NOTE は重大 caveat 限定)。
- `pub` は外部API のみ。モジュール外に呼び出しが無い項目は private/`pub(crate)`。
- すべての `use` はファイル先頭の連続ブロック。インライン完全修飾パス禁止。
- `Query`/`Single` パラメータに `_q` サフィックス禁止。可変パラメータを先に。
- change-detection は条件付き書き込みで駆動(`set_changed`/`bypass` 不使用)。
- 変更後 `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`。
- 末尾で `cargo build` と `cargo test -p ozmux_tmux` が緑。
