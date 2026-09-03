# `Grid::grid_line` を O(1) にする — 履歴索引

webview placement のアンカー解決を、ring 全体の線形走査から履歴索引の
定数時間参照に置き換える設計書。スタック PR の 1 本目で、後続の
`docs/todo/eviction-at-source.md`（退避の発生源報告）がこれを前提にする。
挙動は変えない純粋な最適化であり、変更は `crates/orzma_vt/src/screen/grid.rs`
と、新設する `crates/orzma_vt/src/screen/grid/history_index.rs`（`grid.rs` が
`mod history_index;` で宣言する）の 2 ファイルに閉じる。

## 現状: アンカー解決が ring 長に比例する

placement は `LineId` をアンカーとして持ち、`Grid::grid_line(id)` で現在の
行位置に解決する（`crates/orzma_vt/src/screen/grid.rs:206`）。

```rust
pub fn grid_line(&self, id: LineId) -> Option<GridLine> {
    let index = self.rows.iter().rposition(|row| row.id == id)?;
    ...
}
```

`rposition` は ring を末尾から走査するので、live tail 付近のアンカーは
数ステップで見つかるが、履歴の奥にあるアンカーと、既に ring を離れた
（つまり退避対象の）アンカーは ring 全長を払う。

呼び出し元は 2 つで、どちらも placement 全件に対して `grid_line` を呼ぶ。

| 呼び出し元 | 頻度 | 目的 |
| --- | --- | --- |
| `ScreenPlacements::evict_lost_anchors`（`screen/placements.rs:116`） | 現在は pump ごと。`eviction-at-source` 後はチャンクごと + resize ごと | 解決不能なアンカーを退避 |
| `ScreenPlacements::project`（`screen/placements.rs:87`） | frame 発行ごと（`frame.rs:191`） | placement リストの投影 |

本番値は履歴上限 10,000 行（`bevy_orzma_tty/src/lib.rs:32`）、placement 上限は
overlay slot の 12（`orzma_tty_renderer/src/material.rs:308`）なので、最悪で
一回あたり約 12 万回の id 比較になる。webview が無ければ
`ScreenPlacements::is_empty` で即 return するため、通常のシェル利用では
ゼロコストである。問題になるのは「webview あり + 大量出力」で、4096 バイトの
チャンクごとに掃引するとチャンクの parse と同程度のコストを払いうる。

## なぜ順序に頼れないか

「id は単調増加だから行順とみなせる」「失われるのは古い側から」という
前提で早期停止する案は成立しない。`LineId` の doc（`grid.rs:40` 付近）が
既に「履歴区間だけの二分探索も不健全」と明記しており、コード上の根拠は
次のとおり。

| 経路 | 何が起きるか | 失われる id |
| --- | --- | --- |
| `scroll_up_one` で `top == 0`、履歴上限到達 | `pop_front` で最古の行を捨てる | 最小側 |
| `scroll_up_one` で `top > 0`（DECSTBM の領域スクロール） | 領域先頭の行を取り除き、新 id で領域末尾に差し込む | 中間 |
| `scroll_down_one`（RI など） | 領域末尾の行を取り除き、新 id で領域先頭に差し込む | 中間 |
| `resize_rows` の縮小 | `truncate` で ring 末尾（カーソルより下）を落とす | 最大側 |
| `reset` | 全行 | 全部 |

さらに `ScreenPlacements::mount` は mount 順に `push` し、アンカーは
`cursor_line_id()` なので、テーブル自体も id 順ではない。順序ではなく
**索引**で解く。

## 設計: 履歴区間だけを索引する

ring は性質の違う 2 区間からできている。

| 区間 | 長さ | 行の出入り | リサイクル |
| --- | --- | --- | --- |
| 履歴（先頭 `history_len` 行） | 最大 `max_history` | 新しい端に 1 行ずつ入り、古い端（上限溢れ）か新しい端（伸長時の回収）からしか出ない | されない |
| 表示（末尾 `size.rows` 行） | 数十 | 領域スクロールで途中挿入・削除 | される |

線形走査が高くつくのは履歴区間だけで、そこは「両端でしか出入りしない
deque」である。各行に通し番号を振れば、`ring index = 通し番号 − これまで
pop した数` で位置が出る。先頭が pop されても既存の要素は動かない。
表示区間は並びが自由に変わるので索引せず、画面高さで有界な走査のままに
する。

### `HistoryIndex`

`screen/grid/history_index.rs` に置き、`grid.rs` にだけ見せる。状態は
`seq_of` と `popped` の 2 つだけで、次に振る通し番号は
`popped + seq_of.len()` から導出する。区間の連続性は導出式で構造的に
成り立つので、本番コードに `debug_assert!` は置かない。O(history) の
突き合わせ（各エントリの index が ring 上のその行の id と一致し、件数が
`history_len` と一致する）は `#[cfg(test)]` のヘルパに置き、`Grid` の
テストから呼ぶ。

```rust
/// Where each history row sits, by id, in constant time.
///
/// Only history is indexed: a row enters it at the newest end, leaves
/// it from the oldest end (the cap) or the newest end (a growth
/// reclaiming it), and is never recycled while inside. Each entry
/// therefore gets a running sequence number, and its ring index is
/// that number minus the count popped so far, so a pop moves nothing.
///
/// The visible rows stay unindexed: a region scroll recycles and
/// reorders them freely, and a scan over them is bounded by the screen
/// height rather than the history cap.
///
/// # Invariants
///
/// The live sequence numbers form the contiguous interval
/// `[popped, popped + seq_of.len())`, whose length equals the grid's
/// `history_len`.
#[derive(Debug, Default)]
pub(super) struct HistoryIndex {
    seq_of: HashMap<LineId, u64>,
    popped: u64,
}

impl HistoryIndex {
    pub(super) fn index_of(&self, id: LineId) -> Option<usize> {
        let seq = *self.seq_of.get(&id)?;
        Some(usize::try_from(seq - self.popped).expect("a live entry sits at or past the popped count"))
    }

    pub(super) fn enter(&mut self, id: LineId) {
        self.seq_of.insert(id, self.next_seq());
    }

    pub(super) fn pop_oldest(&mut self, id: LineId) {
        self.seq_of.remove(&id);
        self.popped += 1;
    }

    pub(super) fn reclaim_newest(&mut self, id: LineId) {
        self.seq_of.remove(&id);
    }

    fn next_seq(&self) -> u64 {
        self.popped + u64::try_from(self.seq_of.len()).expect("an entry count fits in u64")
    }
}
```

生きている通し番号は常に連続区間 `[popped, popped + len)` を成す。`enter` は
区間の直後の番号を振り、`pop_oldest` は `popped` を外し、`reclaim_newest` は
区間の末尾を外す。回収で空いた番号を後の `enter` が再利用しても、前の id は
既に取り除かれているので古い対応は残らない。

`Grid` にフィールド `history_index: HistoryIndex` を足す。

### `grid_line`

履歴は索引で、表示は走査で引く。id は grid 内で一意なので `rposition` と
`position` の違いは結果に影響しない。

```rust
pub fn grid_line(&self, id: LineId) -> Option<GridLine> {
    let history = self.history_len();
    if let Some(index) = self.history_index.index_of(id) {
        let line = index as i64 - history as i64;
        return Some(GridLine(i32::try_from(line).expect("a ring index minus its history fits in i32")));
    }
    self.rows
        .range(history..)
        .position(|row| row.id == id)
        .map(|line| GridLine(i32::try_from(line).expect("a screen line fits in i32")))
}
```

### 索引を保守する箇所

行が履歴区間の境界を跨ぐのは 4 箇所だけである。

| 箇所 | 操作 |
| --- | --- |
| `scroll_up_one` の `top == 0` 分岐 | 表示 0 行目が履歴の新しい端に入る。`max_history > 0` のとき `enter(departing)`。上限に達していれば `pop_front` した行を先に `pop_oldest`。`max_history == 0`（alternate 画面）では pop される行が departing そのものなので何もしない |
| `scroll_up_one` の `top > 0` 分岐、`scroll_down_one` | 表示区間内の入れ替えなので無変更 |
| `resize_rows` の伸長 | 回収される `min(growth, history_len)` 行は履歴の新しい端。その id を `reclaim_newest` する。通し番号は `len` から導出するので、複数行を外す順序は結果に影響しない |
| `resize_rows` の縮小 | 表示末尾の `truncate` なので無変更 |
| `reset` | `HistoryIndex::default()` に戻す |

`mint` と `push_blank_row` は表示区間に足すだけなので触らない。

`Screen::resize` は `grid.resize` の前に `scroll_up_one` で必要行数を履歴へ
押し出しており、その分は上の通常経路で索引に入る。`scroll_up_one` で
`top == 0` かつ `bottom < size.rows - 1` の場合も、`below_bottom` が変わるだけで
ring が 1 行伸びて境界が `base` から `base + 1` に動くので、表示 0 行目が履歴に
入る点は同じである。縮小の `truncate` は `old_rows - new_rows` 行しか落とさず、
これは旧表示区間を越えないので履歴には届かない。

Codex による監査（2026-09-03）で、`Grid` の全 mutation と `Screen` の全
呼び出し箇所（`line_feed`、`reverse_index`、`resize`、`reset`、print / erase /
DECALN の cell 書き換え）について上の分類が確認されている。

## 効果と代償

- 掃引と投影の両方が `placements × (ハッシュ 1 回 + 画面高さ以下の比較)`
  になる。12 件 × 50 行なら 600 回程度で、12 万回から二桁以上下がる。
  履歴側の参照は `HashMap` なので厳密な最悪 O(1) ではなく期待 O(1) である。
- 代償は履歴が伸びる行送りごとにハッシュ挿入 1 回、上限溢れ後は挿入と
  削除で 2 回。cell 書き込みに埋もれる量だが、行送りという最頻経路に
  乗ることは確かである。
- メモリは 10,000 エントリで数百 KB。
- 外部依存は増やさない。`std::collections::HashMap` の既定ハッシュで足り、
  キーが `u64` なので必要になれば恒等ハッシュに差し替えられる。

## テスト

`grid.rs` の既存 `mod tests`（`grid.rs:336`）に足す。いずれも `grid_line` の
結果を固定する。

- 履歴上限溢れ後: 最古の id は `None`、生き残りは正しい負の行。
- 伸長で履歴を回収した後: 回収された行の id が表示区間の正しい行に解決する。
- 縮小後: 落ちた行の id は `None`、履歴側は無変更。
- `reset` 後: 以前の id はすべて `None`、索引は空。
- 領域スクロール（`top > 0`）と `scroll_down_one` 後: 取り除かれた id は
  `None`、履歴側は無変更。
- alternate 画面相当（`max_history == 0`）でのスクロール: 索引に何も入らない。

Codex レビューで追加を求められたケース。

- `top == 0` かつ `bottom < size.rows - 1`: 表示 0 行目は履歴に入り、margin より
  下の行は表示に残る。
- 上限溢れ → 伸長回収 → さらに行送り: `pop_oldest → reclaim_newest → enter` で
  通し番号の再利用を通す。
- 一度の伸長で複数行を回収: 新しい側から順に外れる。
- 履歴より大きい伸長: 回収と新規追加が混ざる。
- 履歴ありでの列だけの resize: 索引は無変更。
- `Screen::resize` の縮小で複数回の事前スクロールが要る場合: 上限未満と上限到達の
  両方。
- スクロールバック中の resize: アンカー解決と回収分の offset 補正の両方。
- 履歴ありでの領域スクロール（`top > 0`）と `scroll_down_one`、および
  `max_history == 0` での `scroll_down_one`。
- 履歴ありからの `reset` を繰り返し、各 reset 後に再度行送りする。
- 決定的な混合操作テスト: 上限まで行送り、溢れを繰り返し、領域の上下スクロール、
  伸長、縮小、reset、再行送りを順に行い、**各操作の後**に下の突き合わせヘルパを
  呼ぶ。生存している全 id が正しい行に、退避・リサイクルされた全 id が `None` に
  解決することも各ステップで確認する。

`HistoryIndex` 自身の単体テスト（`history_index.rs`）は、`enter` / `pop_oldest` /
`reclaim_newest` の通し番号算術と `index_of` を `Grid` 抜きで固定する。

`#[cfg(test)]` 専用のヘルパは、`seq_of.len() == history_len` と、索引の各エントリが実際にその ring index の行 id と一致することを突き合わせる。

## スコープ外

- `ScreenPlacements` と `Screen` は無変更。
- 退避 signal の配送経路の変更は `docs/todo/eviction-at-source.md`。
- ハッシャの差し替えは計測で必要になってから。
