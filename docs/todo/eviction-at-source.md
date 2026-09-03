# 退避を発生源で報告する — `Vt::sweep_evictions` の削除

webview placement の退避（`VtSignal::WebviewEvicted`）を、pump ごとの掃引で
拾う形から、アンカーを失わせた操作自身が報告する形に改める設計書。
スタック PR の 2 本目で、`docs/todo/grid-history-index.md`（`grid_line` の
O(1) 化）の上に積む。前提 PR が無いと、チャンクごとの掃引が大量出力時に
parse と同程度のコストを払う。

## 現状: pump が掃引する

`Vt::sweep_evictions`（`crates/orzma_vt/src/lib.rs:128`）は両画面の
`evict_lost_anchors` を呼び、解決不能になったアンカーの placement を取り除いて
`WebviewEvicted` にまとめる。呼び手は `OrzmaTty::pump`
（`crates/orzma_tty/src/lib.rs:274`）だけで、次の順序不変条件を手で守っている。

1. `drain_chunks` で全チャンクを解釈する。
2. `sweep_evictions` を呼び、退避があれば coalescer を arm して
   `pending_signals` に積む。
3. `pending_signals` を取り出す（`ChildExit` は最後）。
4. `frame()` を要求する。

アンカーを失わせる経路と、現在の報告口は次のとおり。

| 発生源 | `interpret` を通る | 現在の報告口 |
| --- | --- | --- |
| 出力で行が履歴上限から押し出される | 通る | 次の pump の掃引 |
| `RIS`（`Executor::reset_device`） | 通る | 次の pump の掃引 |
| alt → primary の flip（`Executor::switch_screen`） | 通る | そのチャンクの `InterpretOutput::signals`（掃引外。`docs/memo/decset.md:90`） |
| `Vt::resize` による縮小 | 通らない | 次の pump の掃引 |
| `Vt::scroll` | 通らない | 発生しない（viewport だけ動く） |
| `Vt::remove_placements` | 通らない | 発生しない（呼び手が id を知っている） |

### 何が問題か

- トレイトに「owner は signal drain と frame の前に掃引を呼べ」という
  手動の不変条件が乗っている。`OrzmaTty` 以外の呼び手や pump の順序変更が
  破りうる誤用クラスである。
- `switch_screen` は発生源で signal を出し、`reset_device` は `ResetTitle` は
  発生源で出すのに取り残した placement は掃引に任せる、という非対称がある。
- webview ありのアイドル時に毎 tick 掃引する。

## 調査結果の要約

Codex と Claude Code Agent の並列調査、および他ターミナル 8 実装
（alacritty、wezterm、Ghostty、kitty、libvterm、foot、VTE、Windows Terminal）
のソース調査の結論。

- **signal と frame が同じ batch に乗る保証は今も無い。** `pump` は
  `pending_signals` を毎回無条件に drain するが、frame は coalescer の期限
  （3ms idle / 12ms cap）まで待つ。退避 signal は今日でも `frame == None` の
  batch で先に届く。
- **ホスト側の消費者はどちらの順でも正しく動く。** `on_webview_evicted`
  （`bevy_orzma_webview/src/webview/mount.rs:456`）は id で despawn するだけ、
  `project_webview_overlays`（同 `:563`）は「signal が先」「frame が先」の
  両方を許容する。
- **本番の resize 呼び出し元は observer ではない。** `src/session/layout.rs`
  の `resize_to_window` system が `handle.resize()` を直接呼ぶ。resize から
  同期的に signal を返す設計にすると、`bevy_orzma_tty/src/signals.rs` の
  private な `VtSignal → Tty*Signal` 変換を root binary 側にも複製する
  ことになる。
- **signal 同士の順序は FIFO の `pending_signals` が保証している。**
  `[Evicted{X}, Mount{X}]` の順序依存は `docs/todo/webview-instance-id.md`
  §5 と `mount.rs` の回帰テストが固定している。
- **他実装はすべて、stream 由来イベントを解析時点で同期的にホストへ渡し、
  ホスト起点の resize をイベント源として扱わない。** 画像・placement の
  喪失を明示イベントで知らせる実装は無く、dirty flag か再描画で気づかせる。
  VTE だけがコア内で pending bits に積み `emit_pending_signals()` で
  一括発行する。

orzma の二層構造は、VT 層（`InterpretOutput` に同期で積む）が多数派の
「同期 emitter」に、`OrzmaTty`（`pending_signals` と `pump`）が VTE 型の
「処理ステップ末尾での一括発行」に対応する。この二層をそのまま保つ。

## 設計

### 方針

- VT 層は操作ごとに結果を同期的に返す。`sweep_evictions` は削除する。
- ホストへの配送口は `pump` 一本のまま。Bevy 層と `resize_to_window` は
  変更しない。
- 契約を明文化する: **退避は次の pump までに、かつその変更を反映する最初の
  frame より前に届く。同じ `PumpOutput` に乗るとは限らない。**

### `Vt` トレイト

| メソッド | 現在 | 変更後 |
| --- | --- | --- |
| `sweep_evictions` | `-> Vec<VtSignal>` | 削除 |
| `interpret` | 退避は掃引に任せる | チャンク末尾で両画面を掃引し `signals` に積む。退避があれば `damaged = true` |
| `resize` | `-> bool` | `-> Option<ResizeChanged>` |
| `scroll` | `-> bool` | 変更なし |
| `remove_placements` | `-> bool` | 変更なし |

```rust
/// What a [`Vt::resize`] that changed the dimensions caused besides
/// the grid change.
///
/// # Invariants
///
/// A resize to the size the grid already has changes nothing and
/// strands nothing, so it returns `None`; this value exists only
/// when the dimensions changed.
pub struct ResizeChanged {
    /// The placements whose anchor row the resize dropped out of
    /// history; empty when every anchor survived.
    pub evicted: Vec<InstanceId>,
}
```

`Option<ResizeChanged>` が成立する根拠は `Screen::resize` の同サイズ早期
return（`screen.rs` の `if old == size { return None; }`）で、寸法が変わらない
resize は grid に触れず、何も取り残さない。`Screen::resize` と
`DeviceState::resize` が既に `Option<DamageSpan>` で「変わったときだけ
`Some`」を返しており、トレイト境界まで同じ語法が通る。

### `OrzmaVt`

```rust
fn resize(&mut self, size: GridSize) -> Option<ResizeChanged> {
    let damage = self.device.resize(size)?;
    self.tracker.stage(damage);
    Some(ResizeChanged {
        evicted: self.device.evict_lost_anchors(),
    })
}
```

`interpret` は変更しない。掃引は `Interpreter::parse` の末尾に入る。

### `Interpreter` / `Executor`

`parse` の末尾には既に cursor 変化の liveness fold がある。その隣で
一度だけ掃引する。

```rust
pub fn parse(&mut self, output: &mut InterpretOutput, device: &mut DeviceState,
             tracker: &mut FrameTracker, chunk: &[u8]) {
    let cursor_before = device.active_screen().cursor();
    let mut executor = Executor { output, sync: &mut self.sync, device, tracker };
    self.parser.parse(chunk, &mut executor);
    executor.sweep_evictions();
    executor.output.damaged |= cursor_before != executor.device.active_screen().cursor();
}

impl Executor<'_> {
    /// Names the placements this chunk stranded — a reset, or rows the
    /// output pushed past the history cap — and raises the chunk
    /// liveness, because a shortened placement list is a frame-visible
    /// section change even when no row was damaged.
    ///
    /// The sweep runs once, after the whole chunk, so a placement the
    /// chunk strands and then re-mounts is updated in place rather
    /// than evicted and re-created.
    fn sweep_evictions(&mut self) {
        let Some(evicted) = VtSignal::evicted(self.device.evict_lost_anchors()) else {
            return;
        };
        self.signal(evicted);
        self.output.damaged = true;
    }
}
```

チャンク末尾で一回にする理由:

1. アンカーが失われるのは parse の中で、parse の前に掃引すると最後の
   チャンクが取り残したものは次の出力まで報告されない。
2. チャンク途中で掃引すると「取り残し → 同 id を再 mount」が退避 + 新規
   mount になる。末尾なら再 mount がテーブル上の placement を更新し直して
   から判定する。
3. `signal()` と `output.damaged` の規約（byte 順、frame に関わる変化は
   自分で liveness を立てる）を持つのは `Executor` であり、liveness を
   立てる箇所を一つの型に保てる。

`switch_screen` は `take_placements` でテーブルから外すので今のまま発生源で
出す。`reset_device` は無変更で、`device.reset()` が解決不能にしたアンカーを
末尾の掃引が拾う。

退避 signal はチャンク内の他の signal の後ろに付く。順序が意味を持つのは
同一 id の `[Evicted, Mount]` だけで、そのケースは末尾掃引なら退避自体が
起きない。

### `OrzmaTty`

```rust
pub fn resize(&mut self, cols: u16, rows: u16) -> OrzmaTtyResult {
    // (degenerate-size gate and pty.resize unchanged)
    let changed = self.vt.resize(GridSize { cols, rows });
    self.absorb_resize(changed);
    Ok(())
}

fn absorb_resize(&mut self, changed: Option<ResizeChanged>) {
    let Some(changed) = changed else {
        return;
    };
    self.coalescer.arm_or_extend(Instant::now());
    if let Some(evicted) = VtSignal::evicted(changed.evicted) {
        self.pending_signals.push(TtySignal::Vt(evicted));
    }
}
```

- `spawn` と `detached` も同じ `absorb_resize` を通し、初期サイズ設定が取り残した
  placement を捨てない。
- `feed_chunk` は無変更。`damaged` が arm を駆動し、signal は
  `pending_signals` へ。
- `pump` から掃引ブロックとその `NOTE` を削除する。`ChildExit` を最後に
  する処理は残す。
- `VtSignal::evicted` は現在 `pub(crate)`。`OrzmaTty` から使うため `pub`
  にする（構築箇所を一つに保つ）。

### 安全網

実行時の `debug_assert!` は置かない。代わりに `Vt::resize` に
`#[must_use = "the evicted placements must reach the owner's signal queue"]` を付け、
戻り値を捨てる呼び手を CI（`-D warnings`）で落とす。`Vt` トレイトの doc に
「アンカーを失わせうる操作はすべて自分の戻り値で名前を挙げ、それ以外は退避しない」を
不変条件として明記する。`OrzmaTty` では `resize`・`spawn`・`detached` の 3 呼び手が
private な `absorb_resize` を通り、退避リストを取りこぼさない。

### 挙動の差

同じ pump 内の別チャンクで「アンカーを失う → 同 id を再 mount」が起きた
場合、現在は pump 末尾の掃引前に更新されるので何も起きないが、変更後は
`[Evicted X, Mount X]` が出て CEF ブラウザが despawn と respawn を経る。
どちらも境界（pump かチャンクか）依存の挙動で、ホストはこの順序を回帰
テストで扱い済み。10,000 行の履歴を越えたアンカーを同 id で再 mount する
アプリは想定しづらく、正しさの問題ではなく respawn 確率のわずかな増加。

## テスト

### `orzma_vt/src/lib.rs`

掃引テスト 4 本を置き換える。

| 現在 | 変更後 |
| --- | --- |
| 何も失っていない掃引は signal を出さない | webview の無い端末に出力を流しても `WebviewEvicted` は出ない |
| reset 後の掃引が名前を挙げる | `RIS` を含むチャンクの `InterpretOutput` が名前を挙げ、`damaged` が真 |
| shrink 後の掃引が名前を挙げる | `resize` の `ResizeChanged::evicted` が名前を挙げる。同サイズは `None` |
| 二度目の掃引は何も出さない | 退避を出した次のチャンクは何も出さない |

追加: 空画面での `RIS`（row damage を積まない）でも `damaged` が真になる。

### `orzma_tty/src/lib.rs`

- `a_pump_reports_what_the_eviction_sweep_raised`（`:461`）:
  「resize してから pump」で退避が `PumpOutput::signals` に乗る形に。
- `an_eviction_arms_the_coalesce_window`（`:481`）: 「resize の退避が
  arm する」と「退避を含む interpret（`damaged = true`）が arm する」の
  2 本に分ける。
- `FakeVt`（`test_support.rs:73`）: `sweeps` を消し、
  `resize_outcomes: VecDeque<Option<ResizeChanged>>` を持たせる。

## ドキュメントの追随

| 箇所 | 変更 |
| --- | --- |
| `Vt::resize` の doc（`orzma_vt/src/lib.rs:151` 付近） | 「取り残しは次の `sweep_evictions` が名前を挙げる」を削除し、`ResizeChanged` を説明 |
| `VtSignal` の doc（同 `:198` 付近） | 「`sweep_evictions` から返る」の一文を削除 |
| `DeviceState::resize` の doc（`device.rs:93`） | 「次の `evict_lost_anchors` が名前を挙げる」を「`Vt::resize` が返す」に |
| `Executor::switch_screen` の doc（`interpreter.rs`） | 「owner の掃引に任せない理由」を「チャンク末尾の掃引に任せない理由」に |
| `docs/todo/webview-instance-id.md:199`、`:425`〜`:446` | `sweep_evictions` 前提の記述を更新 |
| `docs/memo/decset.md:90` | 「pump の掃引では拾えない」を「チャンク末尾の掃引では拾えない」に |
| `orzma_vt/src/lib.rs:279` | 参照先 `docs/orzma_vt_internal_design.md` はワークツリーに無い。今回の変更と独立だが、doc を触るついでに直す |

## スコープ外

- `grid_line` の性能は前提 PR で解決済み。「行が ring を離れたときだけ
  掃引する」watermark gate は前提 PR で不要になったため入れない。
- `Vt::scroll` と `Vt::remove_placements` の戻り値は広げない。
- `OrzmaTty::resize` や `RequestTtyResize` observer から signal を同期的に
  Bevy へ流す設計は採らない。

## レビューでの決定

`docs/superpowers/specs/2026-09-04-eviction-at-source-design.md`（git 管理外）に
基づく変更点。

- `InterpretOutput::signals` の byte 順契約は二段階になる: parser が出した signal は
  byte 順、チャンク末尾の `WebviewEvicted` はその後ろ。
- `FakeVt` は `Option<ResizeChanged>` 全体ではなく `evictions: VecDeque<Vec<InstanceId>>`
  だけを script し、`changed` は従来どおりサイズ比較で決める。
- `ResizeChanged` は `orzma_vt::prelude` から export し、`Debug, Clone, PartialEq, Eq` を
  derive する。
- `interpret` 経路のテストは `lib.rs` ではなく `interpreter/tests/reset.rs` と
  `interpreter/tests/webview_apc.rs` に置き、履歴上限を越える出力の退避テストを追加する。
- `Vt::resize` から `InterpretOutput` を返す案は不採用（`replies` が常に空になり、
  `Option<DamageSpan>` と揃えた「変わったときだけ `Some`」の形が失われる）。
