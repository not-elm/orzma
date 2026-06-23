# TmuxConnection → TmuxClient コンポーネント化 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `tmux -CC` 接続を所有する `NonSend` リソース `TmuxConnection`(`Rc<RefCell<ProtocolClient>>`)を、ゲートウェイ・エンティティ上の `Send + Sync` コンポーネント `TmuxClient` に置き換え、`Rc`/`Option`/`run_if` 不可の3問題を解消する。

**Architecture:** `ProtocolClient` を直接所有する `TmuxClient` コンポーネントをゲートウェイ端末エンティティに同居させる。接続関連の per-connection 状態(`EnumerationState`、attach 状態)も同エンティティのコンポーネントにする。送信側は `Single<&mut TmuxClient>`(grep/将来移行アンカーの型エイリアス `TmuxClientMut`)でアクセスし、`any_with_component::<TmuxClient>` で健全に gate する。退化していた `ConnectionState` enum は `TmuxAttached` マーカーに簡素化し、到達不能な `dialog.rs` を削除する。

**Tech Stack:** Rust 2024 / toolchain 1.95、Bevy 0.18 ECS(`Component`, `Single`, `any_with_component`, `Added`, required components `#[require]`)、`tmux_control::ProtocolClient`。

**設計スペック:** `docs/superpowers/specs/2026-06-23-tmux-connection-redesign-design.md`(spec-review 反映済み)。

## Global Constraints

- コメントは英語のみ。非doc行コメントは `// TODO:` / `// NOTE:` / `// SAFETY:` のみ(`.claude/rules/rust.md`)。`// NOTE:` は重大な caveat 限定。
- `mod.rs` 禁止。`pub` は外部API のみ、モジュール外に呼び出しが無い項目は private。
- すべての `use` はファイル先頭の連続ブロック。インラインの完全修飾パス禁止。
- `Query` パラメータに `_q` サフィックス禁止(`single` を呼ぶなら単数名、反復するなら複数名)。
- `impl`/モジュール内は可視性降順(`pub` → 限定 → private)。可変パラメータを不変パラメータより前に。
- 全システム変更後に `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`。
- 各タスク末尾で `cargo build` と当該クレートのテストが緑であること。
- 外部公開 `pub` 項目には `///` doc コメント必須。

---

## File Structure

新規/変更ファイルと責務:

- `crates/tmux_session/src/connection.rs` — 全面書き換え。旧 `TmuxConnection`/`Adopted`/`AdoptedHandle` を削除し、`TmuxClient` コンポーネント(`ProtocolClient` 直接所有 + キャッシュ + `&mut self` メソッド)、`TmuxAttached` マーカー、`pub type TmuxClientMut` 型エイリアスを定義。
- `crates/tmux_session/src/plugin.rs` — ドライブチェーン(drain/advance/apply/flush/enumeration/capture)をコンポーネントクエリ化、attach エッジを `TmuxAttached` 方式へ、gate を `any_with_component` 化、`TmuxPresence` 削除、`TmuxEventBatch::has_protocol()` 追加。
- `crates/tmux_session/src/enumerate.rs` — `EnumerationState` を `Resource` から `Component` へ。
- `crates/tmux_session/src/state.rs` — 削除(`ConnectionState`/`next_state`)。
- `crates/tmux_session/src/event_pump.rs` — `advance_state` 削除。
- `crates/tmux_session/src/observers.rs` — `on_connection_reset` から per-entity 状態のリセットを除去。
- `crates/tmux_session/src/lib.rs` — エクスポート更新。
- `src/tmux/adopt.rs` — adopt/teardown をコンポーネント insert/despawn へ。`TmuxPresence` 廃止。
- `src/tmux/dialog.rs` — 削除。`src/tmux.rs` — dialog 登録削除。
- `src/tmux/{input,render,forward,copy_mode,mouse,mouse/apply,window_bar_input,webview_tokens}.rs`、`src/ui/{rename_prompt,copy_search,confirm_prompt}.rs` — 送信サイトを `TmuxClientMut`/`Single` へ。
- `src/tmux.rs` — `request_detach` の `&TmuxConnection` → `&mut TmuxClient`。

---

## Task 1: 新コンポーネント型の追加(純増・未配線)

新しい型を追加するだけ。既存の `TmuxConnection`/`EnumerationState`/`ConnectionState` には触れない。ワークスペースはそのままビルドでき、新ユニットテストが緑になる。`#[require(EnumerationState)]` は Task 4 まで付けない(Task 4 で `EnumerationState` が Component 化されるため)。

**Files:**
- Modify: `crates/tmux_session/src/connection.rs`(末尾に新型を追加。旧コードはこの段階では残す)
- Modify: `crates/tmux_session/src/lib.rs`(エクスポート追加)
- Modify: `crates/tmux_session/src/plugin.rs`(`TmuxEventBatch::has_protocol()` 追加)

**Interfaces:**
- Produces:
  - `struct TmuxClient`(`#[derive(Component, Debug, Default)]`)。メソッド:
    - `fn new_adopted() -> Self` — `ProtocolClient::new()` を生成し `register_external_pending()` を呼ぶ(戻り値は破棄)。
    - `fn feed(&mut self, bytes: &[u8]) -> TmuxResult<Vec<ClientEvent>>`
    - `fn take_outgoing(&mut self) -> Vec<u8>`
    - `fn send(&mut self, cmd: impl TmuxCommand) -> TmuxResult<CommandId>`
    - `fn send_raw(&mut self, cmd: &str) -> TmuxResult<CommandId>`
    - `fn client_name(&self) -> Option<&str>` / `fn set_client_name(&mut self, name: String)`
    - `fn supports_per_window_refresh(&self) -> Option<bool>` / `fn set_per_window_refresh(&mut self, supported: bool)`
  - `struct TmuxAttached`(`#[derive(Component, Debug, Default)]`)— マーカー。
  - `pub type TmuxClientMut<'w> = Single<'w, &'w mut TmuxClient>;`
  - `TmuxEventBatch::has_protocol(&self) -> bool` — 保持イベントに `TransportEvent::Protocol` が含まれるか。

- [ ] **Step 1: `connection.rs` に `TmuxClient` のユニットテストを書く(失敗する)**

`crates/tmux_session/src/connection.rs` の `#[cfg(test)] mod tests` に追記:

```rust
    #[test]
    fn tmux_client_send_and_feed_roundtrip() {
        let mut client = TmuxClient::new_adopted();
        let _id = client.send_raw("list-windows").expect("send");
        assert_eq!(client.take_outgoing(), b"list-windows\n");

        let events = client
            .feed(b"\x1bP1000p%begin 1 1 1\r\n%end 1 1 1\r\n")
            .expect("feed");
        assert!(matches!(
            events.as_slice(),
            [ClientEvent::CommandComplete { .. }]
        ));
    }

    #[test]
    fn tmux_client_caches_default_to_none() {
        let mut client = TmuxClient::new_adopted();
        assert_eq!(client.client_name(), None);
        assert_eq!(client.supports_per_window_refresh(), None);
        client.set_client_name("ozmux-0".to_string());
        client.set_per_window_refresh(true);
        assert_eq!(client.client_name(), Some("ozmux-0"));
        assert_eq!(client.supports_per_window_refresh(), Some(true));
    }
```

- [ ] **Step 2: テストが失敗(コンパイルエラー)することを確認**

Run: `cargo test -p ozmux_tmux tmux_client_ 2>&1 | head -20`
Expected: FAIL（`cannot find type TmuxClient` 等のコンパイルエラー）

- [ ] **Step 3: `TmuxClient` / `TmuxAttached` / `TmuxClientMut` を実装**

`connection.rs` の `use` ブロックに `bevy::prelude::*`（`Component`, `Single` を含む）が必要。先頭の `use` を以下に整える（既存の `use` ブロックに `Component`/`Single` を追加。`Entity`/`RefCell`/`Rc` は新コードでは未使用になるが、旧 `TmuxConnection` がまだ残るので Task 1 では削除しない）:

```rust
use bevy::ecs::component::Component;
use bevy::ecs::system::Single;
```

ファイル末尾の `#[cfg(test)]` の直前に追加:

```rust
/// A `tmux -CC` control client owned as a component on the gateway entity.
///
/// Holds the sans-IO [`ProtocolClient`] directly (no `Rc`/`RefCell`): the
/// component is accessed through exclusive `&mut` query access, so interior
/// mutability is unnecessary. Inserted on adoption and removed when the gateway
/// entity is despawned on teardown.
#[derive(Component, Debug, Default)]
pub struct TmuxClient {
    protocol: ProtocolClient,
    client_name: Option<String>,
    per_window_refresh: Option<bool>,
}

impl TmuxClient {
    /// Returns a client for a freshly adopted `tmux -CC` stream.
    ///
    /// Pre-registers the single reply block the adopted stream emits on entry
    /// (its DCS introducer is glued to the first `%begin`) so the in-world drive
    /// correlates it instead of dropping it as unsolicited.
    pub fn new_adopted() -> Self {
        let mut protocol = ProtocolClient::new();
        let _entry = protocol.register_external_pending();
        Self {
            protocol,
            client_name: None,
            per_window_refresh: None,
        }
    }

    /// Feeds a raw byte chunk (from the gateway PTY) through the protocol,
    /// returning the [`ClientEvent`]s it produced.
    pub fn feed(&mut self, bytes: &[u8]) -> TmuxResult<Vec<ClientEvent>> {
        self.protocol.feed(bytes)
    }

    /// Drains the protocol's outgoing buffer for the caller to write back to the
    /// gateway PTY.
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        self.protocol.take_outgoing()
    }

    /// Encodes and queues `cmd`, returning its [`CommandId`].
    pub fn send(&mut self, cmd: impl TmuxCommand) -> TmuxResult<CommandId> {
        self.protocol.send(&cmd.into_raw_command())
    }

    /// Queues an already-rendered command string, returning its [`CommandId`].
    pub fn send_raw(&mut self, cmd: &str) -> TmuxResult<CommandId> {
        self.protocol.send(cmd)
    }

    /// Returns the control client's name as reported by tmux, or `None` if the
    /// name query has not yet completed.
    pub fn client_name(&self) -> Option<&str> {
        self.client_name.as_deref()
    }

    /// Returns whether the attached tmux supports per-window `refresh-client`,
    /// or `None` if the version query has not completed yet.
    pub fn supports_per_window_refresh(&self) -> Option<bool> {
        self.per_window_refresh
    }

    /// Caches the control client name returned by the `display-message` query.
    pub fn set_client_name(&mut self, name: String) {
        self.client_name = Some(name);
    }

    /// Caches the per-window `refresh-client` capability from the version reply.
    pub fn set_per_window_refresh(&mut self, supported: bool) {
        self.per_window_refresh = Some(supported);
    }
}

/// Marks a [`TmuxClient`] entity that has received its first protocol event.
///
/// Inserted on the attach edge (the first `TmuxEventBatch` protocol event after
/// adoption). `With<TmuxAttached>` is the "attached" guard; `Added<TmuxAttached>`
/// is the attach edge for one-shot work.
#[derive(Component, Debug, Default)]
pub struct TmuxAttached;

/// A `Single` query for mutable access to the live [`TmuxClient`].
///
/// Auto-skips the system when there is not exactly one client. A type alias (not
/// a custom `SystemParam`) so the future multi-session migration to `Query` has
/// one named anchor.
pub type TmuxClientMut<'w> = Single<'w, &'w mut TmuxClient>;
```

- [ ] **Step 4: `lib.rs` にエクスポート追加**

`crates/tmux_session/src/lib.rs:31` を以下に変更:

```rust
pub use connection::{AdoptedHandle, TmuxAttached, TmuxClient, TmuxClientMut, TmuxConnection};
```

- [ ] **Step 5: `TmuxEventBatch::has_protocol()` を追加**

`crates/tmux_session/src/plugin.rs` の `impl TmuxEventBatch` 内、`events()` の直後に追加:

```rust
    /// Returns whether this frame's batch contains any protocol event.
    ///
    /// Used by the attach-edge detector instead of re-scanning the batch.
    pub fn has_protocol(&self) -> bool {
        self.0
            .iter()
            .any(|e| matches!(e, TransportEvent::Protocol(_)))
    }
```

- [ ] **Step 6: テストが緑になることを確認**

Run: `cargo test -p ozmux_tmux tmux_client_ -v 2>&1 | tail -20`
Expected: PASS（2テスト）

- [ ] **Step 7: ビルド全体が通ることを確認**

Run: `cargo build 2>&1 | tail -5`
Expected: 成功（警告は新 `pub` 未使用程度。`TmuxClientMut` の型エイリアスがコンパイルできることを確認。もし `&'w mut` でライフタイムエラーが出る場合は `Single<'w, &'static mut TmuxClient>` に変更して再ビルド)

- [ ] **Step 8: コミット**

```bash
cargo fmt
git add crates/tmux_session/src/connection.rs crates/tmux_session/src/lib.rs crates/tmux_session/src/plugin.rs
git commit -m "feat(tmux): add TmuxClient component, TmuxAttached marker, TmuxClientMut alias"
```

---

## Task 2: `EnumerationState` を Resource → Component 化

`EnumerationState` をゲートウェイ・エンティティのコンポーネントにする。`ProtocolClient` はまだ `TmuxConnection` リソースに残る。ドライブ系システムは `connection.gateway()` で得たエンティティの `EnumerationState` を `Single<&mut EnumerationState>` で引く。クレート内で完結(`EnumerationState` をエクスポートし、binary の adopt が insert)。

**Files:**
- Modify: `crates/tmux_session/src/enumerate.rs`（`Resource` → `Component`）
- Modify: `crates/tmux_session/src/lib.rs`（`EnumerationState` をエクスポート）
- Modify: `crates/tmux_session/src/plugin.rs`（`init_resource` 削除、6システムのクエリ化、テスト移植）
- Modify: `crates/tmux_session/src/observers.rs`（`on_connection_reset` から `EnumerationState` リセット除去）
- Modify: `src/tmux/adopt.rs`（adopt で `EnumerationState` を gateway に insert）

**Interfaces:**
- Consumes: Task 1 の型(未使用)。
- Produces: `EnumerationState` が `#[derive(Component)]`。ドライブ系は `Single<&mut EnumerationState>` で参照。

- [ ] **Step 1: `EnumerationState` を Component 化**

`crates/tmux_session/src/enumerate.rs` で `EnumerationState` の定義を探し、`#[derive(... Resource ...)]` を `Component` に置換。`use bevy::prelude::Resource;` を `use bevy::ecs::component::Component;` に変更（他で `Resource` を使っていないことを確認。使っていればそのまま残す）。

Run（定義位置の確認）: `grep -n "Resource\|struct EnumerationState\|derive" crates/tmux_session/src/enumerate.rs`

- [ ] **Step 2: `lib.rs` で `EnumerationState` をエクスポート**

`crates/tmux_session/src/lib.rs:33-36` の `pub use enumerate::{...}` に `EnumerationState` を追加（アルファベット位置は問わない）:

```rust
pub use enumerate::{
    CopyState, EnumerationState, LIST_WINDOWS_FORMAT, WindowRow, absolute_to_visible_row,
    parse_copy_state, parse_window_rows,
};
```

- [ ] **Step 3: `plugin.rs` の `init_resource::<EnumerationState>()` を削除**

`crates/tmux_session/src/plugin.rs:55` の `.init_resource::<EnumerationState>()` 行を削除（チェーンの他行は維持）。

- [ ] **Step 4: ドライブ系6システムを `EnumerationState` クエリ化**

対象システム（すべて `crates/tmux_session/src/plugin.rs`）と変換:
`request_pane_captures`(133), `recapture_settled_panes`(208), `send_attach_enumeration`(277), `send_tmux_reenumeration`(315), `apply_tmux_replies`(431)。各々の `mut enumeration: ResMut<EnumerationState>,` を削除し、代わりに `Single<&mut EnumerationState>` を取る。可変パラメータ先頭ルールに従い、`mut enumeration: Single<&mut EnumerationState>,` を他の可変パラメータ群に並べる。本体で `enumeration` を使う箇所は `Single` の `Deref`/`DerefMut` でそのまま動く（`&mut *enumeration` が必要な箇所のみ調整）。

例（`send_attach_enumeration`、277行付近）:

```rust
fn send_attach_enumeration(
    mut enumeration: Single<&mut EnumerationState>,
    connection: NonSend<TmuxConnection>,
) {
    let Some(handle) = connection.handle() else {
        return;
    };
    let enumeration = &mut **enumeration;
    send_session_enumeration(enumeration, &handle);
    enumeration.register(handle.send(ClientName), PendingReply::ClientName);
    // ...（以降の enumeration.register(...) はそのまま）
}
```

`apply_tmux_replies`(431) は `&mut EnumerationState` を `apply_reply` に渡している。`Single` から `let enumeration = &mut **enumeration;` で `&mut EnumerationState` を取り出して既存の呼び出しに渡す。

NOTE: これらのシステムは `resource_exists::<TmuxPresence>` で gate 済み。接続中はゲートウェイのみが `EnumerationState` を持つので `Single` は解決する。

- [ ] **Step 5: `on_connection_reset` から `EnumerationState` リセットを除去**

`crates/tmux_session/src/observers.rs:228` の `mut enumeration: ResMut<EnumerationState>,` パラメータと、247行付近の `*enumeration = EnumerationState::default();` を削除。理由を doc に追記（リセットは gateway entity の despawn で行われる）。

- [ ] **Step 6: adopt で `EnumerationState` を gateway に insert**

`src/tmux/adopt.rs` の `on_control_mode_detected`、`connection.adopt(gateway);`（136行付近）の直後に追加:

```rust
    commands
        .entity(gateway)
        .insert(EnumerationState::default());
```

`src/tmux/adopt.rs` の `use ozmux_tmux::{...}` に `EnumerationState` を追加。

- [ ] **Step 7: クレートテストを Component 前提へ移植**

`crates/tmux_session/src/plugin.rs` のテストで `EnumerationState` を Resource として扱う箇所を移植:
- `app.init_resource::<EnumerationState>()`（783行付近）→ 削除し、テスト内でゲートウェイ相当エンティティに `EnumerationState::default()` を spawn/insert。
- `app.world().resource::<EnumerationState>()`（643/689/749/804/1105/1127/1157行付近）→ `app.world_mut().query::<&EnumerationState>().single(app.world())` 等でエンティティから取得。`recapture_rearms_after_pane_size_change`(775)は既にゲートウェイ entity を spawn しているので、そこへ `EnumerationState::default()` を insert し、アサートはそのエンティティから取得。
- `apply_reply_client_name_sets_connection_and_seeds_windows`(871) の `SystemState<(..., ResMut<EnumerationState>, ...)>` → ゲートウェイに `EnumerationState` を insert し `Single<&mut EnumerationState>` または `Query` で取得する形へ。
- `observers.rs` のテスト(345 `init_resource`, 等)も同様に移植。

各テストは「ゲートウェイ entity に `EnumerationState` を持たせる」よう統一する。

- [ ] **Step 8: クレートテストが緑であることを確認**

Run: `cargo test -p ozmux_tmux 2>&1 | tail -20`
Expected: PASS（全テスト）

- [ ] **Step 9: ビルド全体が通ることを確認**

Run: `cargo build 2>&1 | tail -5`
Expected: 成功

- [ ] **Step 10: lint & commit**

```bash
cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt
git add -A
git commit -m "refactor(tmux): make EnumerationState a per-gateway Component"
```

---

## Task 3: `ConnectionState` を `TmuxAttached` マーカーへ置換

退化した `ConnectionState` enum を撤去し、`TmuxAttached` マーカーに置換。attach エッジ検出を marker insert + 既存 `TmuxClientAttached` メッセージへ。到達不能な `dialog.rs` を削除。`ProtocolClient` はまだ `TmuxConnection` リソースに残る。

**Files:**
- Modify: `crates/tmux_session/src/plugin.rs`（`advance_tmux_connection` 書き換え、`send_tmux_reenumeration` ガード変更、`init_resource::<ConnectionState>` 削除、テスト移植）
- Modify: `crates/tmux_session/src/observers.rs`（`on_connection_reset` から `ConnectionState` リセット除去）
- Delete: `crates/tmux_session/src/state.rs`
- Modify: `crates/tmux_session/src/event_pump.rs`（`advance_state` と関連 import/テスト削除）
- Modify: `crates/tmux_session/src/lib.rs`（`mod state;` と `pub use state::ConnectionState;` 削除）
- Delete: `src/tmux/dialog.rs`
- Modify: `src/tmux.rs`（`mod dialog;` / `use dialog::DialogPlugin;` / `DialogPlugin,` 登録の3行削除）

**Interfaces:**
- Consumes: Task 1 の `TmuxAttached`、`TmuxEventBatch::has_protocol()`。
- Produces: `advance_tmux_connection` は gateway に `TmuxAttached` を insert し `TmuxClientAttached` を write。`send_tmux_reenumeration` は `With<TmuxAttached>` で attached を判定。

- [ ] **Step 1: attach 検出の新テストを書く(失敗する)**

`crates/tmux_session/src/plugin.rs` のテストに追加。adopt → 初 protocol イベントで `TmuxAttached` が付き、`TmuxClientAttached` が1回発火することを検証:

```rust
    #[test]
    fn first_protocol_event_marks_attached_once() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.insert_resource(TmuxPresence);
        let gateway = app
            .world_mut()
            .spawn(AdoptedControlMode::from_captured(
                b"\x1bP1000p%begin 1 1 1\r\n%end 1 1 1\r\n".to_vec(),
            ))
            .id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);
        app.world_mut()
            .entity_mut(gateway)
            .insert(EnumerationState::default());
        app.update();
        assert!(
            app.world().get::<TmuxAttached>(gateway).is_some(),
            "first protocol event must mark the gateway TmuxAttached"
        );
        let attached = app.world().resource::<Messages<TmuxClientAttached>>();
        assert_eq!(attached.iter_current_update_messages().count(), 1);
    }
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p ozmux_tmux first_protocol_event_marks_attached_once 2>&1 | tail -20`
Expected: FAIL（`TmuxAttached` が付かない / 旧ロジックのまま）

- [ ] **Step 3: `advance_tmux_connection` を `TmuxAttached` 方式へ書き換え**

`crates/tmux_session/src/plugin.rs:257` の `advance_tmux_connection` を以下に置換（gateway entity を `connection.gateway()` で取得し、`Without<TmuxAttached>` を `Query` で確認):

```rust
/// Inserts [`TmuxAttached`] and emits [`TmuxClientAttached`] on the attach edge:
/// the first protocol event in this frame's batch while the gateway is not yet
/// attached. Gated on a pending batch.
fn mark_attached_on_first_protocol(
    mut commands: Commands,
    mut attached: MessageWriter<TmuxClientAttached>,
    connection: NonSend<TmuxConnection>,
    already: Query<(), With<TmuxAttached>>,
    batch: Res<TmuxEventBatch>,
) {
    let Some(gateway) = connection.gateway() else {
        return;
    };
    if already.get(gateway).is_ok() || !batch.has_protocol() {
        return;
    }
    commands.entity(gateway).insert(TmuxAttached);
    attached.write(TmuxClientAttached);
}
```

`plugin.rs` の `use` から `advance_state`（13行付近の `event_pump::{advance_state, ...}`）を削除。`ConnectionState` の import（21行 `use crate::state::ConnectionState;`）を削除。`Build` 内の system 登録(67行付近)で `advance_tmux_connection` を `mark_attached_on_first_protocol` に改名。`.run_if(tmux_batch_pending)` は維持。

- [ ] **Step 4: `send_tmux_reenumeration` のガードを `With<TmuxAttached>` へ**

`crates/tmux_session/src/plugin.rs:315` の `send_tmux_reenumeration`:
- パラメータ `state: Res<ConnectionState>,` を削除し、`attached: Query<(), With<TmuxAttached>>,` を追加。
- 348行付近の `if matches!(*state, ConnectionState::Attached)` を、gateway が `TmuxAttached` を持つ判定に置換:

```rust
    let is_attached = connection
        .gateway()
        .is_some_and(|gw| attached.get(gw).is_ok());
    if is_attached
        && connection.client_name().is_none()
        && !enumeration.has_pending(PendingReply::ClientName)
    {
        enumeration.register(handle.send(ClientName), PendingReply::ClientName);
    }
```

（`enumeration` は Task 2 で `Single<&mut EnumerationState>` になっている。）

- [ ] **Step 5: `init_resource::<ConnectionState>()` 削除と reset 除去**

- `crates/tmux_session/src/plugin.rs:53` の `.init_resource::<ConnectionState>()` を削除。
- `crates/tmux_session/src/observers.rs:231` の `mut state: ResMut<ConnectionState>,` と 254行付近 `*state = ConnectionState::default();` を削除。`ConnectionState` の import も削除。

- [ ] **Step 6: `state.rs` 削除と `advance_state` 削除**

- `crates/tmux_session/src/lib.rs` から `mod state;`(20行) と `pub use state::ConnectionState;`(44行) を削除。
- `rm crates/tmux_session/src/state.rs`
- `crates/tmux_session/src/event_pump.rs`: `advance_state` 関数(22-31行)とその doc、`use crate::state::{ConnectionState, next_state};`(11行)、関連テスト(`advance_state_attaches_on_first_notification` 308行付近)を削除。

- [ ] **Step 7: `dialog.rs` 削除と登録解除**

- `rm src/tmux/dialog.rs`
- `src/tmux.rs` から3行を削除: `mod dialog;`(5行), `use dialog::DialogPlugin;`(23行), プラグイン登録の `DialogPlugin,`(56行)。

- [ ] **Step 8: 旧 `ConnectionState` 参照テストの移植/削除**

`plugin.rs` のテストで `ConnectionState` を使う箇所:
- `plugin_registers_resources_and_stays_idle_without_connection`(632) の `ConnectionState` アサートを削除（または「`TmuxAttached` を持つ entity が無い」アサートに置換）。
- `advance_to_attached_emits_client_attached_message`(653) は新 `first_protocol_event_marks_attached_once`(Step 1) に置換済みなので削除。
- `second_adoption_after_reset_reattaches_and_reenumerates`(1060): `ConnectionState` アサート群を `TmuxAttached` 有無へ移植。reset 後に gateway2 が `TmuxAttached` 無し→初イベントで付与され `TmuxClientAttached` が再発火、を検証。`TmuxConnectionReset` は引き続き発火させるが `ConnectionState` は触れない。
- `drain_transport_clears_stale_batch_once_then_skips_idle`(613) は `ConnectionState` 非依存なら変更不要。

- [ ] **Step 9: クレートテストが緑であることを確認**

Run: `cargo test -p ozmux_tmux 2>&1 | tail -20`
Expected: PASS

- [ ] **Step 10: ビルド全体が通ることを確認**

Run: `cargo build 2>&1 | tail -5`
Expected: 成功（dialog 削除で binary も通る）

- [ ] **Step 11: lint & commit**

```bash
cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt
git add -A
git commit -m "refactor(tmux): replace ConnectionState with TmuxAttached marker; drop dead dialog overlay"
```

---

## Task 4: `ProtocolClient` をコンポーネントへ移設(原子的コア)

`ProtocolClient` を `TmuxConnection` リソースから `TmuxClient` コンポーネントへ移す。これに伴い adopt/teardown・ドライブチェーン・全送信サイト・`TmuxPresence` を一括移行し、`AdoptedHandle`/`Rc`/`RefCell`/旧 `TmuxConnection` を削除する。`ProtocolClient` の所在は二箇所に置けないため、このタスクは crate + binary を横断する原子的変更。テストスイートが安全網。

**Files:**
- Modify: `crates/tmux_session/src/connection.rs`（旧 `TmuxConnection`/`Adopted`/`AdoptedHandle` 削除、`TmuxClient` に `#[require(EnumerationState)]` 追加）
- Modify: `crates/tmux_session/src/plugin.rs`（drain/flush/apply/enumeration/capture を `Single`/`TmuxClientMut` 化、gate を `any_with_component` 化、`TmuxPresence` 削除）
- Modify: `crates/tmux_session/src/lib.rs`（`AdoptedHandle`/`TmuxConnection`/`TmuxPresence` エクスポート削除）
- Modify: `src/tmux/adopt.rs`（adopt=insert、teardown=despawn、`TmuxPresence` 廃止、Task 2 で足した明示 `EnumerationState` insert を削除）
- Modify: 送信サイト群（下記リスト）
- Modify: `src/tmux.rs`（`request_detach` の符号反転、`TmuxPresence` 利用箇所の置換）
- Modify: `src/tmux/webview_tokens.rs`（`bind_tmux_pane_tokens`/`refresh_ozma_sock` の gate を `any_with_component`/`Added` 化）

**Interfaces:**
- Consumes: Task 1-3 の `TmuxClient`/`TmuxClientMut`/`TmuxAttached`/`EnumerationState`(Component)。
- Produces: `TmuxConnection`/`AdoptedHandle`/`TmuxPresence` は消滅。接続の存在判定は `any_with_component::<TmuxClient>`、アクセスは `Single<&mut TmuxClient>`/`TmuxClientMut`。

- [ ] **Step 1: `TmuxClient` に required components を付与**

`crates/tmux_session/src/connection.rs` の `TmuxClient` derive 行を変更:

```rust
#[derive(Component, Debug, Default)]
#[require(EnumerationState)]
pub struct TmuxClient {
```

`use` に `use crate::enumerate::EnumerationState;` を追加。

- [ ] **Step 2: adopt をコンポーネント insert へ**

`src/tmux/adopt.rs` `on_control_mode_detected`:
- `connection.adopt(gateway);` と Task 2 で足した `commands.entity(gateway).insert(EnumerationState::default());` を、`commands.entity(gateway).insert(TmuxClient::new_adopted());` に置換（`EnumerationState` は `#[require]` で自動付与）。
- 既存接続検出: `if let Some(old) = connection.gateway()` を、`existing: Query<Entity, With<TmuxClient>>` パラメータを使った検出に置換:

```rust
fn on_control_mode_detected(
    ev: On<ControlModeDetected>,
    mut commands: Commands,
    mut next_mode: ResMut<NextState<AppMode>>,
    existing: Query<Entity, With<TmuxClient>>,
    ui_root: Query<Entity, With<UiRoot>>,
    containers: Query<Entity, With<DefaultModeUi>>,
) {
    let gateway = ev.entity;
    if let Ok(old) = existing.single()
        && old != gateway
    {
        commands.entity(old).despawn();
        commands.trigger(TmuxConnectionReset);
    }
    commands.entity(gateway).insert(TmuxClient::new_adopted());
    // ...（reparent/despawn container/next_mode はそのまま。TmuxPresence の
    //      remove/insert 行は削除）
    next_mode.set(AppMode::Tmux);
}
```

`NonSendMut<TmuxConnection>` パラメータと `connection.adopt`/`TmuxPresence` 操作を削除。`use` を `TmuxClient` 等へ更新。

- [ ] **Step 3: teardown をエンティティ despawn へ**

`src/tmux/adopt.rs`:
- `on_gateway_child_exit`: `connection.gateway() == Some(ev.entity)` 判定を `clients: Query<(), With<TmuxClient>>` で `clients.get(ev.entity).is_ok()` に置換し、`teardown(&mut commands, ev.entity)` を呼ぶ。
- `teardown_on_exit_notification`: `%exit` を含むとき、`Query<Entity, With<TmuxClient>>` の単一エンティティを teardown。gate を `.run_if(any_with_component::<TmuxClient>)` に。
- `teardown` ヘルパを以下に置換:

```rust
fn teardown(commands: &mut Commands, gateway: Entity) {
    commands.entity(gateway).despawn();
    commands.trigger(TmuxConnectionReset);
    commands.trigger(TmuxConnectionClosed);
}
```

（`connection.close()`/`is_connected()`/`TmuxPresence` 除去は削除。冪等性は呼び出し側の `With<TmuxClient>` 判定で担保。）

- [ ] **Step 4: `sync_gateway_size` の参照を更新**

`src/tmux/adopt.rs:68` `sync_gateway_size`: `connection: NonSend<TmuxConnection>` + `connection.gateway()` を `gateway: Single<Entity, With<TmuxClient>>` に置換（`Single` なので接続無しで自動スキップ）。gate を `.run_if(any_with_component::<TmuxClient>.and(resource_exists::<TerminalCellMetricsResource>))` に。

- [ ] **Step 5: ドライブチェーンを `TmuxClient` クエリ化**

`crates/tmux_session/src/plugin.rs`:
- `drain_tmux_transport`(361): `connection: NonSend<TmuxConnection>` + `adopted: Query<&mut AdoptedControlMode>` を、`client: Single<(&mut AdoptedControlMode, &mut TmuxClient)>` に置換。`connection.gateway()`→単一エンティティ、`control.take_captured()` と `client.feed(&bytes)` を同エンティティから。

```rust
fn drain_tmux_transport(
    mut batch: ResMut<TmuxEventBatch>,
    mut pane_output: MessageWriter<PaneOutput>,
    mut client: Single<(&mut AdoptedControlMode, &mut TmuxClient)>,
) {
    let (control, tmux) = &mut *client;
    let bytes = control.take_captured();
    let drained = match tmux.feed(&bytes) {
        Ok(events) => { /* 既存ロジックそのまま（MAX_EVENTS_PER_FRAME 警告含む） */ }
        Err(error) => { tracing::warn!(?error, "tmux protocol feed failed"); Vec::new() }
    };
    // 以降 collect_pane_outputs / batch 代入は既存どおり
}
```

- `flush_tmux_outgoing`(407): `Single<(Entity, &mut TmuxClient)>` で取り、`take_outgoing()` が空でなければ `TerminalRawWrite { entity, bytes }` を trigger。
- `apply_tmux_replies`(431): `connection: NonSendMut<TmuxConnection>` を `client: Single<&mut TmuxClient>` に置換。`apply_reply` のシグネチャ `connection: &mut TmuxConnection` → `client: &mut TmuxClient`（`set_client_name`/`set_per_window_refresh`/`handle().send` の各呼び出しを `client` のメソッドに置換。`connection.handle()` 経由の送信は `client.send(...)` 直呼びに）。`trigger_notification(..., connection.client_name(), ...)` → `client.client_name()`。
- `send_attach_enumeration`(277)/`send_tmux_reenumeration`(315)/`request_pane_captures`(133)/`recapture_settled_panes`(208): `connection: NonSend<TmuxConnection>` + `connection.handle()` を `Single`/`TmuxClientMut` に置換。`handle.send(cmd)` → `client.send(cmd)`。`request_pane_capture` ヘルパの `handle: &AdoptedHandle` 引数を `client: &mut TmuxClient` に変更し、呼び出しを合わせる。`send_session_enumeration` も同様に `&mut TmuxClient` を取る。
  - 注: `request_pane_captures`/`recapture_settled_panes` は `&mut EnumerationState` と `&mut TmuxClient` の両方を1エンティティから引く → `Single<(&mut TmuxClient, &mut EnumerationState)>` でまとめる。`recapture_settled_panes` は `new_panes: Query<&TmuxPane>` 等のクエリと両立(別コンポーネント)。
- `send_tmux_reenumeration` 内の `connection.client_name()` は `client.client_name()`、`detect_session_switch(..., connection.client_name())` も同様。`commands.trigger(TmuxWindowsRetained{...})` 等は不変。

- [ ] **Step 6: gate を `any_with_component::<TmuxClient>` 化**

`crates/tmux_session/src/plugin.rs` の `Plugin::build`:
- ドライブチェーン(63-76)と capture 系(77-82)の `.run_if(resource_exists::<TmuxPresence>)` を `.run_if(any_with_component::<TmuxClient>)` に置換。
- `mark_attached_on_first_protocol` は `.run_if(tmux_batch_pending)` 維持（チェーンが `any_with_component` で gate 済み）。
- `use bevy::ecs::schedule::common_conditions::any_with_component;`（または `bevy::prelude` 経由）を追加。
- `TmuxPresence` の定義(37行)と `lib.rs:43` のエクスポートを削除。

- [ ] **Step 7: binary 送信サイトを `TmuxClientMut`/`Single` へ一括移行**

各ファイルの `connection: NonSend<TmuxConnection>` + `let Some(handle) = connection.handle() else {...};` を、`Single`/`Option<Single>`/`TmuxClientMut` に置換。送信は `handle.send(x)` → `client.send(x)`。読み取り専用は `Single<&TmuxClient>`。接続無しが正常 no-op の箇所は `Option<Single<&mut TmuxClient>>`。

対象（現行行は目安）:
- `src/tmux/input.rs`(120, 673、`request_detach` 呼び出し 303 付近)
- `src/tmux/render.rs`(550、`supports_per_window_refresh()` 582)
- `src/tmux/forward.rs`(23, 41)
- `src/tmux/copy_mode.rs`(130, 176)
- `src/tmux/mouse.rs`(197、`is_connected()` 274)、`src/tmux/mouse/apply.rs`(34)
- `src/tmux/window_bar_input.rs`(13)
- `src/tmux/webview_tokens.rs`(63、`gateway()` 114)
- `src/ui/rename_prompt.rs`(241)、`src/ui/copy_search.rs`(163)、`src/ui/confirm_prompt.rs`(213)

変換パターン（システムの場合）:

```rust
// Before
fn sys(/* ... */ connection: NonSend<TmuxConnection>) {
    let Some(handle) = connection.handle() else { return; };
    handle.send(SomeCmd { .. }).ok();
}
// After
fn sys(/* ... */ mut client: TmuxClientMut) {
    client.send(SomeCmd { .. }).ok();
}
```

個別注意:
- `src/tmux/mouse.rs:274` の `connection.is_connected()` は decider に渡す bool。`Option<Single<&TmuxClient>>` を取り `client.is_some()` を渡す。`mouse.rs:80` の「`NonSend<TmuxConnection>` を読む run condition は不健全」NOTE は不要になるので削除。
- `src/tmux/webview_tokens.rs:114` の `conn.gateway()?` は、その `TmuxClient` エンティティ自体が gateway。`Single<Entity, With<TmuxClient>>` で取得。
- `src/tmux/render.rs:582` `connection.supports_per_window_refresh()` → `Single<&TmuxClient>` の `client.supports_per_window_refresh()`。
- `Single` を取るシステムは `connection` 無し前提で自動スキップするため、当該システムが他の理由で常時走る必要がある場合は `Option<Single<...>>` を使う(既存の早期 return と等価)。

- [ ] **Step 8: `request_detach` の符号反転**

`src/tmux.rs` の `request_detach(connection: &TmuxConnection)` を `request_detach(client: &mut TmuxClient)` に変更し、本体の送信を `client.send(...)`/`client.send_raw(...)` に。呼び出し元（`src/tmux/input.rs` の `forward_keys_to_tmux`）で `TmuxClientMut`/`Single<&mut TmuxClient>` から `&mut *client` を渡す。

- [ ] **Step 9: `webview_tokens.rs` の gate を Component ベースへ**

`src/tmux/webview_tokens.rs`:
- `bind_tmux_pane_tokens.run_if(resource_exists::<TmuxPresence>)` → `.run_if(any_with_component::<TmuxClient>)`。
- `refresh_ozma_sock.run_if(resource_added::<TmuxPresence>)` → `.run_if(|q: Query<(), Added<TmuxClient>>| !q.is_empty())`。
- テスト内の `app.insert_resource(TmuxPresence)` を、ゲートウェイ entity に `TmuxClient::new_adopted()` を spawn する形へ移植。`Added` ベースの一回起動を検証するテストを保つ。

- [ ] **Step 10: 旧 `TmuxConnection`/`AdoptedHandle` 削除**

`crates/tmux_session/src/connection.rs` から `TmuxConnection` 構造体・`Adopted`・`impl TmuxConnection`・`AdoptedHandle`・`impl AdoptedHandle`、および旧ロジック専用 import（`Entity`, `Rc`, `RefCell`, `ProtocolClient` のうち `TmuxClient` で未使用のもの）を削除。`crates/tmux_session/src/lib.rs:31` を:

```rust
pub use connection::{TmuxAttached, TmuxClient, TmuxClientMut};
```

旧 `connection.rs` のテスト `adopt_then_send_and_feed_roundtrip` を削除(Task 1 の `tmux_client_*` が後継)。

- [ ] **Step 11: 残る crate テストを移植**

`plugin.rs` の以下を `TmuxClient` 前提へ:
- `drive_feeds_captured_bytes`(755)/`transcript_drives_ecs_projection_and_pane_output`(952)/`second_adoption_after_reset_reattaches_and_reenumerates`(1060)/`flush_outgoing_triggers_raw_write_to_gateway`(842)/`apply_reply_client_name_sets_connection_and_seeds_windows`(871)/`recapture_rearms_after_pane_size_change`(775)/`send_attach_enumeration_runs_on_message`(680)/`drain_transport_clears_stale_batch_once_then_skips_idle`(613)。
- 共通移植: `app.insert_non_send_resource(TmuxConnection::default())` + `conn.adopt(gateway)` → `app.world_mut().entity_mut(gateway).insert(TmuxClient::new_adopted())`。`app.insert_resource(TmuxPresence)` は不要（gate が `any_with_component`）。`connection.client_name()` アサート → entity の `&TmuxClient` から取得。
- `src/tmux/adopt.rs` のテスト群(266-658): `connection.adopt`/`is_connected`/`gateway`/`TmuxPresence` を、`TmuxClient` の spawn/有無・entity 取得へ移植。`re_adoption_after_teardown_re_enters_tmux`/`re_adopt_while_live_replaces_and_despawns_old_gateway`/`gateway_sized_to_full_window_on_adopt` 等を新 API で。

- [ ] **Step 12: クレートテストとビルドの確認**

Run: `cargo test -p ozmux_tmux 2>&1 | tail -20`
Expected: PASS
Run: `cargo build 2>&1 | tail -5`
Expected: 成功

- [ ] **Step 13: lint & commit**

```bash
cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt
git add -A
git commit -m "refactor(tmux): move ProtocolClient into TmuxClient component; drop Rc/AdoptedHandle/TmuxConnection/TmuxPresence"
```

---

## Task 5: 検証・整理・ドキュメント更新

**Files:**
- Modify: `CLAUDE.md`（src モジュールマップ/プラグイン列挙の dialog 言及があれば更新）
- 必要に応じ doc コメント補完

- [ ] **Step 1: ワークスペース全体のテスト**

Run: `cargo test 2>&1 | tail -30`
Expected: 全テスト PASS

- [ ] **Step 2: clippy/fmt 最終確認**

Run: `cargo clippy --workspace --all-targets 2>&1 | tail -20`
Expected: 警告なし(あれば修正)
Run: `cargo fmt --check`
Expected: 差分なし

- [ ] **Step 3: 残存参照の grep 確認**

Run: `grep -rn "TmuxConnection\|AdoptedHandle\|TmuxPresence\|ConnectionState\|advance_state\|DialogPlugin\|dialog" --include="*.rs" src/ crates/`
Expected: 旧名の本番参照ゼロ（テスト/コメントにも残骸が無いこと。あれば除去）

- [ ] **Step 4: doc コメント監査**

新規 `pub` 項目（`TmuxClient`, `TmuxAttached`, `TmuxClientMut`, `TmuxEventBatch::has_protocol`）に `///` doc があることを確認。`connection.rs`/`plugin.rs` のファイル先頭 `//!` を新責務に合わせて更新（`connection.rs` の `//!` が「`NonSend` リソース」を述べていれば「ゲートウェイ・エンティティ上の `TmuxClient` コンポーネント」に修正）。

- [ ] **Step 5: CLAUDE.md 更新**

`CLAUDE.md` のプラグイン列挙や crate 説明で `TmuxDialogPlugin`/dialog や `ConnectionState` に言及があれば、削除/更新。`crates/tmux_session` の説明に `ConnectionState` lifecycle 記述があれば `TmuxClient`/`TmuxAttached` ベースへ更新。

Run: `grep -n "Dialog\|ConnectionState\|TmuxConnection" CLAUDE.md`

- [ ] **Step 6: 手動スモークテスト(任意・推奨)**

Run: `cargo run`（`tmux -CC` を起動し、ウィンドウ/ペイン投影・入力転送・detach 復帰が動くか目視）。

- [ ] **Step 7: コミット**

```bash
git add -A
git commit -m "docs(tmux): update CLAUDE.md and module docs for TmuxClient redesign"
```

---

## Self-Review メモ

- **Spec coverage**: Rc 除去(Task 4)/Option・run_if(Task 4 gate)/Component 化(Task 1,4)/EnumerationState 同居(Task 2)/ConnectionState→TmuxAttached(Task 3)/TmuxPresence 撤去(Task 4)/required components(Task 4 Step 1)/TmuxClientMut 型エイリアス(Task 1)/request_detach 符号反転(Task 4 Step 8)/has_protocol(Task 1)/Added one-shot(Task 4 Step 9) — 全項目にタスク対応あり。
- **原子性**: `ProtocolClient` の所在は一意のため Task 4 は crate+binary 横断の原子的変更。Task 1-3 は各境界でビルド緑(共有型 EnumerationState/ConnectionState を先に移動)。
- **型整合**: `TmuxClient::send`/`feed`/`take_outgoing`/`client_name`/`set_client_name`/`set_per_window_refresh`/`new_adopted` の名称は Task 1 定義と Task 4 利用で一致。`TmuxClientMut`/`TmuxAttached`/`has_protocol` も一貫。
