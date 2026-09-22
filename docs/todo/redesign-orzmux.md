# orzmux モジュール再編: バックエンドとイベントループの分離

## 目的

`crates/orzmux` を「マルチプレクサの業務ロジック」と「スレッド間通信のロジック」に
分ける。現状の `Backend`（`crates/orzmux/src/backend.rs:29`）は 14 フィールドで
両方を抱えている:

| 分類 | フィールド |
| --- | --- |
| 業務 | `factory`, `panes`, `tree: LayoutTree`, `geometry`, `window_focused`, `next_pane_id`, `wheel`, `processed` |
| 通信 | `commands: Receiver`, `events: Sender`, `gui_gone`, `sources: Vec<Ready>`, `sampler` |

`run()` の `Select` ループと `handle_command` のディスパッチは通信側に属するが、
業務メソッドから `self.emit()` が 7 箇所（`backend.rs` の 204 / 352 / 446 / 496 /
540 / 544 / 560 行）で直接呼ばれているため、両者が分離できていない。

## 完了条件

1. `backend.rs` と `backend/*` に `crossbeam_channel` の import がない。
2. `backend/*` から `crate::event_loop` への import がない（依存が `event_loop → backend` の一方向）。
3. `cargo test -p orzmux` が通り、現行テストが検証している契約が 1 つも失われていない。
4. `orzmux::prelude` の公開シンボル集合が変わらない。`bevy_orzmux` / `src/` に差分が出ない。

## モジュール構成の差分

| 現在 | 変更後 | 内容 |
| --- | --- | --- |
| `src/layout.rs` | `src/backend/layout.rs` | 移動のみ。`orzmux::layout` を import する外部クレートは存在しないので `pub(crate) mod` に落とす |
| `src/backend/queue_sample.rs` | 変更なし | モジュールは backend 側に残す。3 つの入力のうち per-pane の chunk depth は backend 所有であり、その chunk channel は `orzma_tty` のリーダースレッド内部で GUI 境界ではない。GUI 境界のチャネルは `events` / `commands` の 2 本だけ。`QueueSampler` の**所有**だけ `EventLoop` に移し、`ChunkDepth` は境界型のまま保つ |
| `src/backend/pane.rs` | 変更なし | |
| `src/backend.rs` | 縮小 + 型の受け入れ | `run` / `wait_ready` / `next_wake_deadline` / `drain_commands` / `handle_command` / `emit` / `record_queue_depths` / `report_queue_sample` / `enum Ready` / `const COMMAND_BATCH` が抜ける。`protocol.rs` の大半を受け入れる |
| — | `src/event_loop.rs`（新規） | `EventLoop` + 上で抜いたもの + `OrzmuxCommand` |
| `src/protocol.rs` | 削除 | 内訳は「型の配置」参照 |
| `src/client.rs` | import 差し替え | `use crate::backend::{Backend, ShellFactory}` → `EventLoop`。spawn するのは `EventLoop::new(..).run()` |
| `src/lib.rs` | 全モジュールを `pub(crate) mod` に落とし、prelude からのみ公開 | `pub mod layout` / `pub mod protocol` 削除、`pub(crate) mod event_loop` 追加 |
| `src/error.rs` | 変更なし | |
| `.claude/rules/rust.md`, `CLAUDE.md` | 記述を更新 | `rules.md:774` の「Protocol purity — `orzmux::protocol` types carry no...」が存在しないモジュールを指すことになる。`CLAUDE.md:22` のクレート説明も同様 |

**`backend.rs` は小さくならない。** 抜ける非テストコードは約 100 行、受け入れる protocol
型は約 210 行で、差し引きほぼ現状の 1883 行のまま（うち約 1200 行が `#[cfg(test)]`）。
ファイルサイズが動機に含まれるなら、型の移動ではなく 18 個の操作ハンドラを
`backend/ops.rs` に分けるかテストモジュールを分割する方が効くが、それは本再編の
目的ではないので別タスクとする。

### `lib.rs` の公開範囲

現状 `pub mod backend; pub mod client; pub mod error; pub mod layout; pub mod protocol;`
だが、`Backend` 自体は `pub(crate)` で、外部クレートの import は全て
`orzmux::prelude::` 経由であることを確認済み（`orzmux::backend::` / `orzmux::protocol::`
/ `orzmux::layout::` の直接参照はゼロ）。モジュールを private にして prelude だけを
公開面にする。private モジュール内の `pub` 項目を `pub use` で再エクスポートするのは
ファサードの通常形で、完了条件 4 を満たす（`rustc --edition 2024` の最小再現で確認済み。
glob 再エクスポートは各項目を小さい方の可視性に丸めるため、`pub` と `pub(crate)` が
混在していても通る）。

`orzmux` は `Cargo.toml:78` の `[workspace.package]` で `publish = false` なので、
crates.io 下流の利用者は存在しない。パスの互換性を保つ facade モジュールは不要。

再エクスポートは **glob ではなく明示列挙**にする。クレート最大かつ最も変更が入る
`backend.rs` からの glob は、完了条件 4 を「今日は真、明日は黙って壊れる」状態にする
（`backend.rs` に `pub` 項目が 1 つ増えただけでクレートの公開 API に加わる）。

```rust
pub(crate) mod backend;
pub mod client;
pub mod error;
pub(crate) mod event_loop;

pub mod prelude {
    pub use crate::backend::{
        CloseReason, CommandSeq, Layout, NewPaneAt, OrzmuxEvent, PaneDirection, PaneId,
        PaneRect, PaneTarget, RequestId, Separator, SplitId, SplitOrientation,
    };
    pub use crate::client::{OrzmuxClient, OrzmuxConfig};
    pub use crate::error::*;
    pub use crate::event_loop::OrzmuxCommand;
}
```

## 型の配置

### 一方向依存の制約

`protocol.rs` を丸ごと `event_loop.rs` に統合すると依存が逆流する。理由は 3 つ:

1. `backend/layout.rs` が `PaneDirection` / `PaneId` / `PaneRect` / `Separator` /
   `SplitId` / `SplitOrientation` を使う。
2. `backend/queue_sample.rs` が `PaneId` を使う。
3. **Backend が `OrzmuxEvent` 値を組み立てる。** `pump_pane` → `forward_items` は
   pump の item 列を `Signal` / `Frame` に 1 対 1 で並べ替えて出す。ここを
   `OrzmuxEvent` 以外の型で返すと `OrzmuxEvent` の写しを作ることになる。
   したがって `OrzmuxEvent` は backend 側の語彙。

逆に `OrzmuxCommand` は `EventLoop::handle_command` が分解して `Backend` の
メソッドに割り当てるだけで、`Backend` は一度も触らない。よって
**`protocol.rs` から `event_loop.rs` へ移るのは `OrzmuxCommand` 1 つだけ**になる。

### 配置表

| 型 | 移動先 | 根拠 |
| --- | --- | --- |
| `PaneId`, `SplitId` | `backend.rs` | レイアウト木と pane 台帳の識別子 |
| `WindowId` | **削除** | workspace 全体で定義（`protocol.rs:23`）以外の参照がゼロ。死んだ `pub` 型を、削る対象のファイルへ移す意味はない。タブを見越した型なら `TODO:` で足りる |
| `SplitOrientation`, `PaneDirection` | `backend.rs` | `backend/layout.rs` が使う |
| `PaneRect`, `Separator`, `Layout` | `backend.rs` | `LayoutTree::solve` の出力 |
| `PaneTarget`, `NewPaneAt` | `backend.rs` | `pane_id` / `pinned_pane_at` の入力語彙 |
| `CloseReason` | `backend.rs` | `close_pane` の引数 |
| `RequestId` | `backend.rs` | `OrzmuxEvent::PaneOpened` / `SpawnFailed` が持つ |
| `CommandSeq` | `backend.rs` | `Layout.seq` と `Backend.processed` が持つ |
| `OrzmuxEvent` | `backend.rs` | Backend が値を組み立てる（上記 3） |
| `OrzmuxCommand` | `event_loop.rs` | Backend は触らない |

`protocol.rs` の 2 つのテスト（`request_ids_are_strictly_increasing`,
`the_default_layout_has_no_panes`）は `backend.rs` のテストモジュールへ移す。

`protocol.rs` にあった `assert_send_static` の `const` ガードは置き直さない。
`.claude/rules/rust.md` の「Static assertions」がこの形を禁じており、
`OrzmuxEvent` / `OrzmuxCommand` の `Send + 'static` は `OrzmuxClient::spawn` が
`thread::Builder::spawn` に渡すクロージャが既に強制している。

`Layout.seq: CommandSeq` は `bevy_orzmux` が古い layout の棄却に使っている
（`crates/bevy_orzmux/src/layout.rs:258`, `requests/pane.rs:69`）ので、`Layout` の
形は変えない。`CommandSeq` を backend 側に置くことで越境が消える。

## Backend の公開表面

`EventLoop` が呼ぶメソッドは 4 群。`Backend` は `pub(crate) struct` なので、`impl` 内の
メソッドは `pub(crate)` ではなく `pub` で書く（`.claude/rules/rust.md` の
「Visibility — don't restate a type's own ceiling on its members」）。型側の可視性が
既に到達範囲を封じているため、どちらでも届く範囲は変わらない。

**操作（`OrzmuxCommand` の各 variant に 1 対 1）** — 現行 `handle_command` の
match アームをそのまま切り出す。現行の `on_resize` / `on_new_pane` は
イベントハンドラ名なので操作名に改める:

```
resize, open_pane, kill_pane, select_pane, select_pane_direction, window_focus,
resize_split, key_input, paste, mouse_input, wheel, scroll,
selection_start, selection_update, selection_clear, copy_selection,
remove_placements, mount_placement
```

`new_pane` ではなく `open_pane` にする。`Backend::new`（コンストラクタ）とも既存の
private な `Backend::spawn_pane`（`backend.rs:423`）とも紛らわしいため。

操作メソッドは**失敗を自分でログに落とさず `OrzmuxResult` で返す**（`.claude/rules/rust.md`
の「Error handling — return `Result`, don't assert or unwrap」。回復できない境界が
ログに落とす、という形）。境界は `handle_command` で、現行の `resolve_or_log` の
debug 行と `log_refused_write` の呼び分けを 1 箇所に集約した `log_refused_command` が
引き受ける。失敗しうる 13 個が `OrzmuxResult`、残り 5 個（`resize` /
`select_pane_direction` / `window_focus` / `resize_split` / `copy_selection`）は `()`。
常に `Ok` を返す `OrzmuxResult` は書かない。

PTY 書き込みの拒否を上流へ返すため、`OrzmuxError` に
`PtyWrite { pane: PaneId, what: &'static str, source: OrzmaTtyError }` を足す。
既存の `#[from] OrzmaTtyError` は `SpawnShell` なので、`?` でそのまま上げると
spawn 失敗に化けてしまう。

`NewPane` の失敗はログではなく `SpawnFailed` というプロトコル上の応答なので、
`open_pane` の `Err` を境界が `fail_spawn` に渡す。`fail_spawn` は `pub` に上げる。

**アドレス解決の改名** — `resolve` / `resolve_at` は何を何に変えるのかを言って
いないので、役割を 2 段に分けて名前を付け直す。

| 現在 | 変更後 | 役割 |
| --- | --- | --- |
| `resolve(target: PaneTarget)` | `pane_id(target: PaneTarget) -> OrzmuxResult<PaneId>` | アドレス → id |
| `resolve_or_log(target, command)` | **削除** | ログは境界が持つ |
| `pane_mut(target, command)` | `pane_mut(id: PaneId) -> OrzmuxResult<&mut Pane>` | id → 状態 |
| `resolve_at(at: NewPaneAt)` | `pinned_pane_at(at) -> OrzmuxResult<PinnedPaneAt>` | アドレス → 固定済みの配置 |
| 型 `ResolvedPaneAt` | 型 `PinnedPaneAt` | |

`pin` を採るのは、この型が存在する理由そのものが現行の `// NOTE:` に書かれているため
（`NewPaneAt::Split` は `PaneTarget::Active` を運べ、後で再解決すると別の pane を
分割してしまうので、この時点で具体的な id に固定する）。

`live_pane` にはしない。`close_pane` が `tree` と `panes` の両方から消すので、
死んだ pane が `panes` に居座ることはない。`contains_key` が弾いているのは GUI から
届いた古い id であって liveness ではない。

**伝播させないもの**: `publish_layout` と `refresh_focus` の per-pane ループ。
前者は「サイズ変更に失敗した pane は旧サイズのまま次へ進む」、後者は「1 つの pane が
拒否しても他への通知を止めない」が明文の契約なので、途中で `?` すると契約が壊れる。
この 2 つは現行どおり吸収する。

**ポンプ** — `pump_pane(id)`, `service_deadlines()`。どちらも `pane.tty` を回すので
Backend に残る。`const PUMP_ROUNDS` も残る。

**レディネス** — `Select` の構築と待ち期限の計算は `EventLoop` 側だが、データは
Backend 所有。次の 3 つを生やす:

```rust
pub fn readiness(&self) -> impl Iterator<Item = (PaneId, Readiness<'_>)>;
pub fn next_deadline(&self, now: Instant) -> Option<Instant>;
pub fn chunk_depths(&self) -> impl Iterator<Item = (PaneId, ChunkDepth)>;
pub fn pane_deadline(&self, id: PaneId, now: Instant) -> Option<Instant>;
```

`Readiness<'a>` は `orzma_tty` の公開型（`crates/orzma_tty/src/lib.rs:110`）。
`Readiness` は `backend.rs` の先頭 import ブロックに加える（インラインの完全修飾パスを
書かない、という `.claude/rules/rust.md` の規約）。

`queue_sample` を backend 側に残すので `chunk_depths` は `ChunkDepth` をそのまま返せる。
`next_deadline` は全 pane の min を返すため、単一 pane の期限を見たいテストには使えない。
`pane_deadline` はそのために要る（`the_wake_deadline_…` が `h.backend.panes[&root].tty
.next_deadline(now)` を読んでいる）。

この 4 つで `EventLoop::wait_ready` / `next_wake_deadline` / `record_queue_depths` は
成立する。借用形状も検証済み: `Readiness<'_>` が `&self.backend` を `Select` の構築中
ずっと保持したまま `&mut self.sources` を書けるのは、両者が独立したフィールドだから
（現行 `backend.rs:262-281` と同じ形）。

**アウトボックス** — `outbox: Vec<OrzmuxEvent>` フィールドと
`pub fn drain_events(&mut self) -> impl Iterator<Item = OrzmuxEvent> + '_`。
`Drain<'_, _>` を戻り値に書かないのは、呼び手を `Vec` のイテレータ型に固定せず、
実装の詳細を名指しするためだけの `use std::vec::Drain;` を増やさないため。

`emit` をシンク引数（`&mut Vec<OrzmuxEvent>`）にせずフィールドにする理由は、
呼び出しが `forward_items` ← `pump_pane` ← `service_deadlines` と 3 段ネストして
おり、シンクを通すと全署名が汚れるため。

**`processed` の扱い** — `handle_command` 冒頭の `self.processed = seq` は
`EventLoop` 側に残らない。`publish_layout` が `Layout.seq` に刻むため
Backend の状態であり、`pub fn set_processed(&mut self, seq: CommandSeq)` を
`EventLoop::handle_command` の冒頭で呼ぶ。

**`gui_gone` は Backend から消える。** 送信失敗を知るのは `EventLoop` だけ。

## `EventLoop`

```rust
pub(crate) struct EventLoop {
    backend: Backend,
    commands: Receiver<(CommandSeq, OrzmuxCommand)>,
    events: Sender<OrzmuxEvent>,
    /// Set when the GUI's event receiver is gone; the loop exits.
    gui_gone: bool,
    /// What each `Select` index of the last `wait_ready` referred to.
    sources: Vec<Ready>,
    /// Per-queue peaks between samples; logged once a second.
    sampler: QueueSampler,
}
```

`QueueSampler` と `ChunkDepth` は `crate::backend::queue_sample` から import する
（`event_loop → backend` の許された向き）。

`run()` は現行と同じ骨格で、各周回の末尾に outbox のフラッシュが入る:

```rust
pub fn run(mut self) {
    loop {
        let ready = self.wait_ready();
        self.record_queue_depths();
        let connected = match ready {
            Some(Ready::Commands) => self.drain_commands(),
            Some(Ready::Pane(pane)) => {
                self.backend.pump_pane(pane);
                true
            }
            None => true,
        };
        self.backend.service_deadlines();
        self.flush_events();
        self.report_queue_sample(Instant::now());
        if !connected || self.gui_gone {
            return;
        }
    }
}
```

`flush_events` が `backend.drain_events()` を回して `events.send` し、失敗したら
`gui_gone` を立てる。

**切断パスでも必ずフラッシュする。** `drain_commands` は溜まったコマンドを適用して
（その各々が `emit` しうる）、同じ呼び出しの中で `Disconnected` を観測しうる。素直に
`Some(Ready::Commands) if !self.drain_commands() => return` と書くと、直前に生成した
イベントを送らずに抜ける。現行の `emit` は即時 `send` なのでこの取りこぼしは起きない。
上のように戻り値を局所変数で持ち、`flush_events()` の後で判定する。実運用では GUI が
去る途中だが、`OrzmuxClient::detached()` はコマンド受信端をテストに渡すので、イベントを
読みながら送信端を落とすテストが書ける。

**バッチングが変えるもの / 変えないもの**: `flush_events()` は次の `wait_ready()` が
ブロックする前に走るので、イベントがブロックを跨いで保持されることはない。レイテンシ、
`CopySelection` の応答、layout / frame の交錯はいずれも不変。本番のチャネルは
`unbounded` なので `emit` は元々ブロックしておらず、バックプレッシャーの変化もない。
sampler が読む `events.len()` も、周回 N の末尾に流したイベントは周回 N+1 の先頭で
チャネルに載っているので現行と同じ。

**outbox の最大長は `COMMAND_BATCH` では抑えられない。** 同一周回に `PUMP_ROUNDS` 回の
ポンプと `service_deadlines` の全 pane 分が積まれ、`forward_items` は 1 回の pump の
`Vec<PumpItem>` を丸ごと展開する。さらに各 `Layout` イベントが `Vec<(PaneId, Frame)>` を
内包する。メモリ上の関心はコマンド件数ではなくフレーム量。

## 移行手順

各ステップ単独で `cargo test -p orzmux` が通ること。**step 0 が要になる** — テスト
ハーネスを先に動かせる形にしておかないと、step 4 以降が単独で緑にならない。

0. **テストハーネスを昇格する。** `FakeFactory` / `FactoryLog` / `FakePane` / `Harness`
   / `open_root` / `split_active` / `settle_writes` を
   `#[cfg(any(test, feature = "test-support"))] pub(crate) mod test_support;` へ出す
   （`orzma_tty::test_support` と同じ形。`backend.rs:698` が既にそれを消費している）。
   同時に、ハーネスが `Backend` の private フィールドに触っている 19 箇所を
   `pub` アクセサ経由に置き換える（`pane_deadline` / `panes_len` /
   `contains_pane` / `tree` ビュー）。`orzmux` の `test-support` feature は現状
   `orzma_tty` へ転送するだけなので、ここで実体を持つ。
1. `src/layout.rs` → `src/backend/layout.rs`。`backend.rs` に `pub(crate) mod layout;`、
   `lib.rs` の `pub mod layout;` を削除。import パスの修正のみ。
2. `WindowId` を削除し、空の `src/event_loop.rs` を作る。
3. `protocol.rs` から `OrzmuxCommand` 以外を `backend.rs` へ移動。`protocol.rs` には
   `OrzmuxCommand` だけ残す。prelude を明示列挙に更新。
4. `OrzmuxCommand` を `event_loop.rs` へ移し、`protocol.rs` を削除。
   `.claude/rules/rust.md` の Protocol purity 項と `CLAUDE.md:22` を同じコミットで更新する。
5. `Backend` に `outbox: Vec<OrzmuxEvent>` を足し、`emit` を `outbox.push` に変更。
   **同じステップで `Harness::drain()` を `backend.drain_events()` 読みに書き換える。**
   これを step 8 に後回しにすると、約 50 本のテストが空のチャネルを読んで落ちる
   （`Harness::drain` は `self.events.try_iter()` を読み、`run()` を呼ぶテストは 1 つも
   ない）。`gui_gone` は当面 `Backend` に残す。
6. `EventLoop` を作り、`run` / `wait_ready` / `next_wake_deadline` / `drain_commands` /
   `handle_command` / `record_queue_depths` / `report_queue_sample` / `enum Ready` /
   `COMMAND_BATCH` / `gui_gone` / `sources` / `sampler` を移す。Backend にアクセサを生やす。
7. `on_resize` / `on_new_pane` を `resize` / `open_pane` に改名し、残りの match アームを
   Backend のメソッドとして切り出す。**同じステップでテスト内の `OrzmuxCommand::` 構築
   38 箇所を直接の操作呼び出しに変換する** — `handle_command` が `EventLoop` に移った
   時点で `Harness::send(OrzmuxCommand)` のディスパッチ先が `Backend` から消えるため。
8. テストの仕分け（下記）。
9. `client.rs` を `EventLoop::new(..).run()` に差し替える。

step 6 と 9 の間、`client.rs` は古いコンストラクタを指したままになる。`Backend::new` の
シグネチャを step 6 で変えるなら、step 6 と 9 は 1 コミットにまとめる。

## テストの仕分け

`backend.rs` は 1884 行中およそ 1200 行が `#[cfg(test)]`。`Harness` が commands /
events チャネルを直接握り `event_kinds` でイベント列を検証しているため、機械的な
移動はできない。基準:

- **Backend 側へ**: 1 コマンドの適用結果だけを見るもの。`Harness` を
  `backend.drain_events()` を読む形に書き換える。イベント順序を見る
  `a_closed_update_frame_arrives_between_its_signals` /
  `the_last_frame_precedes_the_pane_close` / `an_open_update_holds_the_pane_frame_back`
  は outbox が `Vec` なのでそのまま Backend 側で成立する。
- **EventLoop 側へ**: チャネル・`Select`・周回順序を見るもの。
  `record_queue_depths_sees_the_chunks_queued_before_the_pump`、
  `the_wake_deadline_is_the_report_deadline_when_no_pane_deadline_is_earlier`、
  `a_pane_that_stops_reading_does_not_freeze_the_backend`。
- `active_targets_resolve_in_command_order` は **Backend 側**。`drain_commands` も
  `COMMAND_BATCH` も `Select` もチャネルも触っておらず（`Harness::send` は
  `backend.handle_command` を直接呼ぶ）、pin しているのは `PaneTarget::Active` が
  適用時点の `tree.active()` に対して解決されること、つまり `Backend::pane_id` である。
- `a_backend_thread_failure_keeps_the_os_error_text` は `OrzmuxError::BackendThread` の
  `Display` アサーションのみで `OrzmuxClient` を構築しない。`error.rs` へ移す。

EventLoop 側の 3 本は `Backend` の内部に触っている。step 0 のアクセサで賄う:
`the_wake_deadline_…` は単一 pane の期限を読むので `pane_deadline`、
`a_pane_that_stops_reading_…` は `settle_writes` と pane の生存確認、
`record_queue_depths_…` は `open_root` と `pump_pane`。`Harness` 一式が backend と
event_loop の**両方**のテストモジュールから届く必要があり、`backend::tests` の
private 項目のままには置けない。

## 検討して却下した案

**`handle_command` を Backend に残し、`EventLoop` はチャネルと `Select` だけ持つ。**
差分は小さいが、`OrzmuxCommand` が backend 側に残り、Backend が通信の語彙を
話し続ける。本再編の目的そのものを満たさない。

**`Layout` から `seq` を外し `OrzmuxEvent::Layout { seq, layout, frames }` にする。**
層としては綺麗だが `bevy_orzmux` の 2 箇所の判定とテスト群に波及する。
`CommandSeq` を backend 側に置けば同じ効果が波及なしで得られる。

**`protocol.rs` を残して共有スキーマにする。** 決定として却下するが、spec-review で
Codex と Claude Code Agent が**独立に、両者とも簡素化案の第一推奨**として挙げた点なので
根拠を残す。曰く、完了条件 4 つ（backend の crossbeam import / backend の event_loop
import / テスト / prelude シンボル）を plain-data な `protocol.rs` は 1 つも破らない。
一方この分割では `OrzmuxEvent` と `OrzmuxCommand` という対の型が別ファイルに分かれ、
「ワイヤスキーマはどこにあるか」の答えが「2 箇所、値を誰が構築するかで決まる」になる。
`protocol.rs` を残せば `rules.md` の Protocol purity 項と `CLAUDE.md:22` が真のまま
保たれ、prelude も `pub use crate::protocol::*` のままで済む。
一方向依存は完了条件が要求しているものではなく、共有スキーマの依存が
V 字（`backend → protocol ← event_loop`）になるのは当然、という指摘。
それでも統合するのは、語彙の所在が 3 モジュールに散ることを避けるため。

**`Backend` に `Box<dyn EventSink>` を持たせ、`emit` を即時のまま保つ。** `emit` が
即時なら切断時の取りこぼしもバッチングの議論も発生せず、`backend.rs` に
`crossbeam_channel` の import がない点も満たせる。却下する理由は `gui_gone` の所有で、
sink が握ると `EventLoop` が `backend.sink_gone()` 越しに問い合わせる形になり、outbox 案
より配管が増える。テスト側の利得も見かけより小さく、`Harness::drain` の書き換えは
どちらの案でも同じだけ必要。outbox 案の実害は切断パスの 1 行に収まる。

**`Select` を周回を跨いで使い回す。** できない。`Select<'a>` は登録した各
`&'a Receiver` を借りるので、pane を足し引きする `&mut Backend` の変更を跨げず、
持続させるには自己参照型が要る。毎回組み直すのが正しい。

## 未決事項

1. **型名の綴り**: 原案は `Eventloop`。Rust の慣習とファイル名 `event_loop.rs` に
   揃えるなら `EventLoop`。本書は `EventLoop` で記述している。
2. ~~**操作メソッド 18 個の粒度**~~ — 決着。18 個のまま残す。まとめる唯一の手は
   `pane_mut(id) -> OrzmuxResult<&mut Pane>` を公開して `handle_command` から
   `p.tty.scroll(..)` を直接叩く形だが、それは `Pane` と `OrzmaTty` を event_loop 側に
   露出させることであり、この分割が止めようとしている語彙の漏れそのもの。薄さは
   継ぎ目の位置が正しい証拠であって、匂いではない。
3. **`RequestId` の置き場所**: backend は値を mint せず `OrzmuxEvent` に echo する
   だけ（`RequestId::next()` の呼び手は GUI 側）。`OrzmuxEvent` が backend.rs に
   ある以上 backend.rs に置くのが最小だが、意味的には通信側の相関 ID。
4. **`.claude/rules/rust.md` の細目**: (a) `src/event_loop.rs` はファイルレベルの
   モジュールとして `//!` を負う。(b)「private items last」
   により、Backend に増える約 20 個の `pub` 操作は `pane_id` / `pinned_pane_at` /
   `insert_pane` / `spawn_pane` / `publish_layout` / `refresh_focus` / `forward_items` /
   `close_pane` / `pane_mut` / `emit` より**上**に宣言する。
5. **`Select::ready()` は spurious success を返しうる。** 現行は `drain_commands` が
   `try_recv` を使い `pump_pane` が空キューで no-op になることで吸収している。移動の
   際に `wait_ready` を assert で「締めない」こと。

## 原案（依頼時の記述）

以下のようにバックエンドのビジネスロジックと、スレッド間通信のロジックを切り出すようなモジュール構成にしたい。

- backend
  - layout.rs
- event_loop.rs(protocol.rsのスキーマもこのファイルに統合)

```rust
pub(crate) struct Eventloop{
  backend: Backend,
  commands: Receiver<(CommandSeq, OrzmuxCommand)>,
  events: Sender<OrzmuxEvent>,
}

impl Eventloop{
  pub fn run(mut self){ ... }

  fn handle_command(&mut self, command){
    match command {
      OrzmuxCommand::Resize { size, cell_px } => self.backend.resize(size, cell_px),
      ...
    }
  }
}
```
