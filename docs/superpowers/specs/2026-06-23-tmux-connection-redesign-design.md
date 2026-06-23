# TmuxConnection 再設計 — `TmuxClient` コンポーネント化

- **日付**: 2026-06-23
- **対象**: `crates/tmux_session`(主に `connection.rs` / `plugin.rs` / `state.rs` / `event_pump.rs` / `observers.rs`)と binary 側 `src/tmux/*` の送信サイト
- **ステータス**: 設計承認済み(実装プラン未作成)

## 背景と動機

現在 `crates/tmux_session/src/connection.rs` の `TmuxConnection` は、`tmux -CC`
接続を所有する **`NonSend` リソース**である。`docs/memo.md` が挙げる3つの問題:

1. **`Rc` の使用**: `protocol: Rc<RefCell<ProtocolClient>>`。これは
   `AdoptedHandle`(送信専用の安価なクローン可能ハンドル)を成立させるためだけに
   存在する。`Rc<RefCell>` が non-`Send` なので、`TmuxConnection` は `NonSend`
   リソースを強制されている。
2. **常在リソースだが接続は常在しない**: `TmuxConnection` は常に存在するが接続は
   `Option<Adopted>`。結果として `handle()` は `Option` を返し、利用側は毎回存在
   チェックを書く。さらに `NonSend` を読む run condition は不健全
   (bevyengine/bevy#21230)なため `run_if` で gate できず、各システムが body-guard
   (`let Some(handle) = ... else { return; }`)を持つ。
3. **単一リソースゆえ multi-session 不可**: 単一の `TmuxConnection` リソースでは
   将来複数の tmux セッション同時接続を表現できない。

### 鍵となる技術的事実

`ProtocolClient`(`crates/tmux_control/src/protocol.rs`)は
`assembler / line_buf / pending / next_id / outgoing` という素データのみで構成され、
**それ自体は `Send + Sync`** である。`TmuxConnection` を `NonSend` にしている唯一の
原因は `Rc<RefCell<…>>` ラッパであり、ハンドルパターンを廃して `ProtocolClient` を
直接 `&mut` アクセスで触れば、`Send + Sync` な **コンポーネント**にできる。つまり
上記3問題は一つの結び目であり、`Rc` 除去がコンポーネント化の前提となる。

## ゴールとスコープ

### ゴール

- `Rc` / `RefCell` / `AdoptedHandle` を廃止する(問題1)。
- 接続が無い間はコンポーネントが存在しない設計にし、`Option` チェックと body-guard を
  排除、健全な `run_if`(クエリ条件)で gate する(問題2)。
- 接続まわりの状態を **per-connection 化**(コンポーネント化)し、multi-session を
  「`Single` → `Query` に替えるだけ」の地点まで構造的に近づける(問題3)。

### スコープ方針(確定事項)

- **リファクタ優先・単一接続のまま**。実際の複数同時接続 UI/ロジックは今作らない。
- **per-connection 化の範囲 = B(接続に密結した状態まで)**:
  - **コンポーネント化する**: `TmuxClient`(`ProtocolClient` + クライアントキャッシュ)、
    `EnumerationState`。
  - **当面グローバル Resource のまま**: `TmuxProjection`(id→entity 索引)、
    `KeyBindings`、`CopyModeQueries`、`TmuxEventBatch`。
- **`TmuxClient` はゲートウェイ・エンティティに同居**(案1)。専用の接続エンティティは
  作らない。
- **`ConnectionState` は撤去し `TmuxAttached` マーカーに簡素化**(後述)。

### 非ゴール(明示的に範囲外)

- multi-session の実 UI / 複数接続の同時ドライブ。
- エラー UX(`dialog.rs` の "tmux unavailable" / "Disconnected" オーバーレイ)の
  再接続。これは `ConnectionState` の close/error 経路再導入という別テーマであり、
  本リファクタとは関心が分離されている(下記「ConnectionState の判断」参照)。
- `TmuxProjection` / `KeyBindings` / `CopyModeQueries` の per-connection 化。

## 設計

### 1. コンポーネント & データモデル

新コンポーネント `TmuxClient`(`Send + Sync`、ゲートウェイ・エンティティに同居):

```text
TmuxClient {
    protocol: ProtocolClient,        // Rc/RefCell を剥がして直接所有
    client_name: Option<String>,
    per_window_refresh: Option<bool>,
}
```

- `feed` / `take_outgoing` / `send` / `send_raw` は `&mut self`。クエリの排他
  アクセスで足りるため `RefCell` は不要。
- コンストラクタ(例 `TmuxClient::new_adopted()`)が内部で `ProtocolClient::new()`
  を生成し `register_external_pending()` を呼ぶ。現状 `adopt` が返していた
  `CommandId` は実フローでどこにも記録されず捨てられているため、戻り値は廃止する。
- `gateway: Entity` フィールドは消滅(コンポーネント自身がゲートウェイ上にある)。
- `AdoptedHandle` 型は削除。

`EnumerationState` を Resource → **Component** 化(ゲートウェイ・エンティティに同居)。
読み手は crate 内 `plugin.rs` 各システムと `on_connection_reset` のみで外部依存が
無く、安全に entity に載せられる。

当面グローバル Resource のまま: `TmuxProjection` / `KeyBindings` /
`CopyModeQueries` / `TmuxEventBatch`。

### 2. `ConnectionState` の判断 — `TmuxAttached` マーカーへ簡素化

**現状認識**: `ConnectionState` enum は5値(`Idle/Connecting/Attached/Detached/
Error`)だが、本番コードで実際に到達するのは `Idle` と `Attached` の2値のみ。

- `drain_tmux_transport` は feed 結果を**全て `TransportEvent::Protocol` に包む**ため、
  `next_state` が `Detached`/`Error` を返す唯一の入口 `TransportEvent::Closed` が
  来ない。
- `Connecting`/`Detached`/`Error` を本番で設定する箇所は**存在しない**(`dialog.rs`
  は読むだけ、`state.rs`/`event_pump.rs`/`plugin.rs` の該当はテスト)。
- `dialog.rs`(`Detached`/`Error` でモーダル表示)は `in_state(AppMode::Tmux)` か
  つ `ConnectionState` が当該値になる必要があるが、teardown は即 `AppMode::Default`
  へ抜けるため**到達不能**。これは死にコードというより「close/error 経路は将来
  再導入」前提の意図的スタブ(`advance_tmux_connection` の NOTE)。

**決定**: `ConnectionState` の実仕事は (a) attach エッジの1回検出と (b)「attached
か?」ガードの2つだけなので、per-connection の **`TmuxAttached` マーカー
コンポーネント**に置換する。

- `TmuxAttached`: 接続エンティティに付くマーカー。初 protocol イベント到達時に付与。
- attach エッジ検出(旧 `advance_tmux_connection`): 「接続エンティティが未
  `TmuxAttached` かつ今フレームの `TmuxEventBatch` に Protocol イベントあり」→
  `TmuxAttached` を挿入し、`TmuxClientAttached` メッセージを送出。
- `TmuxClientAttached` メッセージは**残す**(実績ある仕組み)。
  `send_attach_enumeration` は `on_message::<TmuxClientAttached>` のまま。
- `send_tmux_reenumeration` の `matches!(*state, Attached)` ガード → `With<TmuxAttached>`。

**削除対象**:
- `crates/tmux_session/src/state.rs`(`ConnectionState` / `next_state` 全体)。
- `crates/tmux_session/src/event_pump.rs::advance_state`(およびそのテスト)。
- `crates/tmux_session/src/lib.rs` の `pub use state::ConnectionState`。
- `src/tmux/dialog.rs` 全体、および `src/tmux.rs` の登録3箇所
  (`mod dialog;` / `use dialog::DialogPlugin;` / プラグイン登録行)。

### 3. ライフサイクル & `TmuxPresence` 撤去

**adopt**(`on_control_mode_detected`):

- `connection.adopt(gateway)` → `commands.entity(gateway).insert((
  TmuxClient::new_adopted(), EnumerationState::default()))`。
  `TmuxAttached` はここでは付けない(初 protocol イベントで付く)。
- 既存接続の検出 `connection.gateway()` → `Query<Entity, With<TmuxClient>>`。
  存在しかつ新ゲートウェイと異なれば、旧エンティティを despawn し
  `TmuxConnectionReset` をトリガ。
- 現状の「`TmuxPresence` を remove → insert して `Added` を再アームするハック」
  (adopt.rs:129-134)は**不要になり削除**。毎 adopt で新エンティティのため
  `Added<TmuxClient>` が自然に再発火する。

**`TmuxPresence`(マーカー Resource)を完全撤去**:

- `resource_exists::<TmuxPresence>` gate → `any_with_component::<TmuxClient>`。
- `resource_added::<TmuxPresence>`(`refresh_ozma_sock` の1回限り起動)→
  `Added<TmuxClient>` ベースの run condition(例:
  `|q: Query<(), Added<TmuxClient>>| !q.is_empty()`)。
- 影響ファイル: `plugin.rs`(drain chain と capture 系の gate)、`adopt.rs`
  (insert/remove・`sync_gateway_size`・`teardown_on_exit_notification` の gate)、
  `webview_tokens.rs`(`bind_tmux_pane_tokens` と `refresh_ozma_sock` の gate)。

**teardown**:

- `connection.close()` + despawn → 接続エンティティ(= ゲートウェイ)を despawn。
  全コンポーネント(`TmuxClient`/`EnumerationState`/`TmuxAttached`)が一緒に消える。
- `on_gateway_child_exit`: `connection.gateway() == Some(ev.entity)` →
  `ev.entity` が `TmuxClient` を持つか(クエリ)で判定し、その entity を teardown。
- `teardown_on_exit_notification`: batch の `%exit` スキャンは不変。gate を
  `any_with_component::<TmuxClient>` に。
- `TmuxConnectionReset` / `TmuxConnectionClosed` のトリガは不変。冪等性は
  `With<TmuxClient>` の有無で担保。

**`on_connection_reset`**(observers.rs):

- entity 同梱状態(`EnumerationState`・旧 `ConnectionState`)のリセットが不要に
  (entity ごと despawn されるため)。グローバル(`TmuxProjection` /
  `KeyBindings` / `CopyModeQueries` / `TmuxEventBatch`)のみクリアする。
- 「`ConnectionState` を `Idle` に戻して再 adopt で `Idle→Attached` を畳ませる」
  ハックは不要(毎 adopt で `TmuxAttached` 無しの fresh entity になるため)。

### 4. アクセスパターン & 健全な gating

**送信サイト(約25システム/オブザーバ)**: `NonSend<TmuxConnection>` +
`connection.handle()` を全廃。

- 通常システム: `Single<&mut TmuxClient>`(接続が無ければシステムを**自動スキップ**
  = `Option` チェックと body-guard が消える)。読むだけなら `Single<&TmuxClient>`。
- オブザーバ: `Single` の自動スキップ挙動が不確実なため、薄い `SystemParam` ラッパ
  `TmuxClientMut`(内部 `Query<&mut TmuxClient>` + `.single_mut().ok()`)を1つ用意し、
  systems / observers 両方をこれ経由に統一。将来の `Single → Query`(multi-session)
  変更がこの一点に閉じる。
- `drain` / `flush` / `apply` / capture 系は同一エンティティを
  `Single<(&mut AdoptedControlMode, &mut TmuxClient, &mut EnumerationState)>` 等で
  引く。`connection.gateway()` 探索が消える。
- 旧 API の対応:
  - `connection.is_connected()` → `With<TmuxClient>` の有無 / `Single` の解決可否。
  - `connection.gateway()` → クエリ対象のエンティティそのもの。
  - `connection.client_name()` / `supports_per_window_refresh()` → `TmuxClient`
    フィールドのアクセサ。

**run condition**: 旧 drain chain を `any_with_component::<TmuxClient>` で gate。
これは通常のクエリ条件であり、現コードの「`NonSend` を読む run_if は不健全
(bevy#21230)」制約を回避する。`src/tmux/mouse.rs` の関連 NOTE(80行付近)も不要に。

### 影響を受けるファイル(blast radius)

- **crate** `crates/tmux_session/src/`:
  - `connection.rs`(全面書き換え: `TmuxClient` / `TmuxClientMut`、`AdoptedHandle`
    と `Rc/RefCell` 削除)
  - `plugin.rs`(drain/apply/enumeration/capture 系のクエリ化、gate 変更、attach
    エッジ簡素化、`TmuxPresence` 削除)
  - `state.rs`(削除)
  - `event_pump.rs`(`advance_state` 削除、`TmuxAttached` 判定ヘルパ追加)
  - `observers.rs`(`on_connection_reset` の簡素化)
  - `lib.rs`(エクスポート更新: `TmuxClient`/`TmuxAttached`/`TmuxClientMut` 追加、
    `ConnectionState`/`TmuxPresence`/`AdoptedHandle` 削除)
- **binary** `src/`(`NonSend<TmuxConnection>` 利用 ≈18 システム):
  - `tmux/adopt.rs`、`tmux/input.rs`、`tmux/render.rs`、`tmux/forward.rs`、
    `tmux/copy_mode.rs`、`tmux/mouse.rs`、`tmux/mouse/apply.rs`、
    `tmux/window_bar_input.rs`、`tmux/webview_tokens.rs`、`ui/rename_prompt.rs`、
    `ui/copy_search.rs`、`ui/confirm_prompt.rs`
  - `tmux/dialog.rs`(削除)、`tmux.rs`(dialog 登録削除)

## テスト

- 既存 crate テストを新 API へ移植:
  - `transcript_drives_ecs_projection_and_pane_output` / `drive_feeds_captured_bytes`:
    エンティティ構築を「ゲートウェイに `TmuxClient` + `EnumerationState` を insert」へ。
  - `second_adoption_after_reset_reattaches_and_reenumerates`: `ConnectionState`
    assertion → `TmuxAttached` 有無へ。reset 後に `TmuxAttached` が無く、再 adopt の
    初イベントで再付与されて再列挙されることを検証。
  - adopt ライフサイクル群(`adopt.rs` tests): `is_connected()` → `With<TmuxClient>`、
    `TmuxPresence` assertion → `any_with_component::<TmuxClient>` / `Added` へ。
- `state.rs` / `event_pump.rs::advance_state` / `dialog.rs` のテストは削除。
- 新規: `TmuxAttached` の付与エッジが「未 attached + Protocol イベント」で1回だけ
  発火し `TmuxClientAttached` を送ること。`TmuxClientMut` が0件/1件で正しく振る舞うこと。

## 移行順序(各ステップでビルド・テストが通る状態を維持)

1. `TmuxClient` / `TmuxAttached` / `TmuxClientMut` を新設(旧 `TmuxConnection` と併存)。
2. crate 内 drain / apply / enumeration / capture 系を新 API(クエリ)へ移行。attach
   エッジを `TmuxAttached` 方式へ。
3. binary 側 send-site を `TmuxClientMut` 経由へ一括移行。
4. adopt / teardown を component 挿入 / despawn ベースへ。
5. `TmuxPresence` を撤去(gate を `any_with_component` / `Added<TmuxClient>` へ)。
6. 旧コード削除: `ConnectionState` / `state.rs` / `event_pump.rs::advance_state` /
   `dialog.rs`(+登録)/ `AdoptedHandle` / `Rc` / `register_external_pending` の戻り値。

## multi-session に向けて残る作業(将来・本スコープ外)

- `TmuxProjection`(id→entity 索引)・`KeyBindings`・`CopyModeQueries`・
  `TmuxEventBatch` の per-connection 化。
- 送信サイトの `Single<&mut TmuxClient>` / `TmuxClientMut` を `Query` 化し、対象
  接続をどう選ぶか(アクティブ接続の概念)を導入。
- close/error 経路とエラー UX(旧 `dialog.rs` 相当)の再設計。

## 確定した実装方針(曖昧さ排除)

- **送信サイトは全て `TmuxClientMut` 経由に統一**する。通常システムで素の `Single`
  を使い observer のみラッパにする折衷は採らない(将来の `Query` 化を一点に閉じ込め、
  systems/observers の挙動差を無くすため)。
- **`refresh_ozma_sock` は `Added<TmuxClient>` を使う run condition で gate** する
  (`|q: Query<(), Added<TmuxClient>>| !q.is_empty()`)。システム本体での `Added`
  クエリ判定は採らない(gate 表現に揃える)。
