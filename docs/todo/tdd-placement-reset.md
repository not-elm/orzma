## テストケース一覧

| # | 名前 | Source | 優先度 |
| - | - | - | - |
| TC-01 | `a_reset_drops_every_placement_and_names_them` | C3 + LC | High |
| TC-02 | `a_reset_drops_a_placement_whose_anchor_left_the_viewport` | C3 + LC | High |
| TC-03 | `a_reset_drops_a_placement_on_the_inactive_screen` | C3 + LC | High |
| TC-04 | `a_reset_names_a_placement_whose_anchor_already_left_the_grid_buffer` | C3 + LC | High |
| TC-05 | `a_second_reset_reports_no_eviction` | C3 + LC | Medium |
| TC-06 | `a_reset_frees_every_slot_the_cap_counts` | CAP + C3 | Medium |
| TC-A1 | `a_reset_does_not_rewind_the_placement_id_counter` | PI | High（実装上の罠） |

Source タグ — **C3**: ECMA-48 p.69 L3214（初期状態への復帰）／**LC**: `VtSignal::WebviewEvicted` と
`evict_lost_anchors` の doc が定めるライフサイクル契約／**CAP**: `MAX_PLACEMENTS` の doc／
**PI**: `PlacementId` の invariant。

TC-01 から TC-06 までが仕様契約 C3 とそのライフサイクル契約に対応し、TC-A1 だけがマニュアル由来ではない。

テストコードは `crates/orzma_vt/src/placement.rs` の `mod tests` へ追加する。既存の `device()` /
`mount()` ヘルパをそのまま使う。`pub(crate) fn reset(&mut self) -> Vec<PlacementId>` を前提に
しているので、**現状の木に対してはコンパイルできない。**

## TC-01 — 全 placement を破棄し、その id を名指す

| | |
| - | - |
| Setup | `device()`; `mount(&mut store, &device, "a")`; `mount(&mut store, &device, "b")` |
| Act | `let evicted = store.reset();` |
| Expect | テーブルが空 **[C3]** ／ `evicted` が 2 つの id ちょうどを名指す **[LC]** |

返り値の順序は契約しない。`assert_eq!(evicted, vec![a, b])` と直接比較せず、`sort()` を挟んで
集合として比較する。

```rust
/// Asserts that a reset empties the placement table and names every id
/// it dropped.
///
/// Case: a program has two webviews mounted on the screen when the user
/// runs `reset(1)`, which sends `ESC c`.
#[test]
fn a_reset_drops_every_placement_and_names_them() {
    let device = device();
    let mut store = PlacementStore::new();
    let first = mount(&mut store, &device, "a").expect("first mount accepted");
    let second = mount(&mut store, &device, "b").expect("second mount accepted");

    let mut evicted = store.reset();

    evicted.sort();
    assert_eq!(evicted, vec![first, second]);
    assert_eq!(store.len(), 0);
}
```

## TC-02 — viewport の外だが grid buffer には在る anchor

| | |
| - | - |
| Setup | `device()`; `mount(&mut store, &device, "memo")`; `device.active_mut().line_feed()` を 3 回。先に `project(..)[0].viewport_row == -1` を確認する |
| Act | `let evicted = store.reset();` |
| Expect | テーブルが空 **[C3]** ／ `evicted` が当該 id を名指す **[LC]** |

`line_feed()` が 3 回でなければならない理由: 1・2 回目はカーソルが row 0 → 1 → 2 と動くだけで
スクロールが起きず、anchor は可視のまま（`viewport_row == 0`）。3 回目でカーソルが最下行にいるため
`scroll_up_one(top=0, bottom=2)` が走り、`top == 0` かつ `history_len(0) < max_history(10)` なので
出ていった行が履歴の最新行になる。

この状態の placement は「テーブルに生きていて、射影もされていて、ただし画面の上にはみ出している」
という中間状態にある。`project_into` は負の row をそのまま renderer に渡し（`placement.rs:78-79`）、
`evict_lost_anchors` は `viewport_row_of` が `Some(-1)` を返すので落とさない。
TC-02 が防ぐのは、reset が sweep の述語（`viewport_row_of(..).is_none()`）を流用する実装。
その実装はこの placement だけを取りこぼし、`Grid::reset()` が履歴を捨てた後に anchor が宙に浮く。
画面からは消えるが host 側の entity は次の sweep まで生き残り、しかもその sweep は
アクティブスクリーンしか見ない（`placement.rs:172-173`）。

```rust
/// Asserts that a reset drops a placement whose anchor left the viewport
/// but still sits inside the grid buffer.
///
/// Case: a webview was mounted beside a line of output that has since
/// scrolled up into the history, and the shell sends `ESC c`.
#[test]
fn a_reset_drops_a_placement_whose_anchor_left_the_viewport() {
    let mut device = device();
    let mut store = PlacementStore::new();
    let id = mount(&mut store, &device, "memo").expect("mount accepted");
    for _ in 0..3 {
        device.active_mut().line_feed();
    }
    assert_eq!(store.project(device.active_screen())[0].viewport_row, -1);

    let evicted = store.reset();

    assert_eq!(evicted, vec![id]);
    assert_eq!(store.len(), 0);
}
```

## TC-03 — 非アクティブスクリーンの placement

| | |
| - | - |
| Setup | `device()`; `set_active_screen_for_test(ScreenKind::Alternate)`; `mount(.., "alt")`; `set_active_screen_for_test(ScreenKind::Primary)`; `mount(.., "prim")` |
| Act | `let evicted = store.reset();` |
| Expect | テーブルが空 **[C3]** ／ `evicted` が alternate と primary 両方の id を名指す **[LC]** |

`switch_screen` との決定的な差分。あちらは Primary の placement を「隠すだけで破棄しない」。

```rust
/// Asserts that a reset drops the inactive screen's placements as well as
/// the active screen's, rather than hiding them the way an
/// alternate-screen flip does.
///
/// Case: a full-screen application mounted a webview on the alternate
/// screen and died without restoring the primary; the shell that takes
/// over sends `ESC c`.
#[test]
fn a_reset_drops_a_placement_on_the_inactive_screen() {
    let mut device = device();
    let mut store = PlacementStore::new();
    device.set_active_screen_for_test(ScreenKind::Alternate);
    let alternate = mount(&mut store, &device, "alt").expect("alternate mount accepted");
    device.set_active_screen_for_test(ScreenKind::Primary);
    let primary = mount(&mut store, &device, "prim").expect("primary mount accepted");

    let mut evicted = store.reset();

    evicted.sort();
    assert_eq!(evicted, vec![alternate, primary]);
    assert_eq!(store.len(), 0);
}
```

## TC-04 — anchor が既に grid buffer から消えている placement

| | |
| - | - |
| Setup | `device()`; `line_feed()` を 2 回; `mount(.., "memo")`; `reverse_index()` を 3 回。先に `project(..)` が空であることを確認する |
| Act | `let evicted = store.reset();` |
| Expect | `evicted` が当該 id を名指す **[LC]** ／ テーブルが空 **[C3]** |

TC-02 と TC-04 は sweep の述語をまたいで両側に置かれている。TC-02 の anchor は `Some(-1)` で
sweep が残し、TC-04 の anchor は `None` で sweep が落とす。両方あって初めて
「reset はその述語を一切見ない」が固定される。

防ぐのは、projection 結果だけを列挙する実装と、sweep の返り値を破棄してから clear する実装。
`evict_lost_anchors` 自体は id を返すので、返り値を連結すれば sweep 先行でも漏れない。

なお `scroll_down_one`（reverse index）は行を `remove` して新しい `LineId` を振り直して再挿入する
（`grid.rs:202-207`）。消えるのは行の同一性であって確保領域ではない。

```rust
/// Asserts that a reset names a placement whose anchor already left the
/// grid buffer, rather than dropping it unreported.
///
/// Case: a webview's anchor row was recycled by a reverse scroll and no
/// eviction sweep has run yet when `ESC c` arrives.
#[test]
fn a_reset_names_a_placement_whose_anchor_already_left_the_grid_buffer() {
    let mut device = device();
    let mut store = PlacementStore::new();
    device.active_mut().line_feed();
    device.active_mut().line_feed();
    let id = mount(&mut store, &device, "memo").expect("mount accepted");
    for _ in 0..3 {
        device.active_mut().reverse_index();
    }
    assert!(store.project(device.active_screen()).is_empty());

    let evicted = store.reset();

    assert_eq!(evicted, vec![id]);
    assert_eq!(store.len(), 0);
}
```

## TC-05 — 2 度目の reset は何も報告しない

| | |
| - | - |
| Setup | `device()`; `mount(.., "memo")`; `store.reset()` |
| Act | `let evicted = store.reset();` |
| Expect | `evicted` が空 **[LC]** ／ テーブルが空 **[C3]** |

空の store に対する reset のケースと setup 以外が同一なのでマージした。
`ris.md` #5 が「返り値が空でなければ chunk liveness を立てる」で分岐する**予定**なので、
空返しは契約になる（`esc_dispatch` に RIS のアームはまだ無い）。

```rust
/// Asserts that a reset of an already-empty table reports no eviction.
///
/// Case: the user runs `reset` twice in a row at a fresh prompt.
#[test]
fn a_second_reset_reports_no_eviction() {
    let device = device();
    let mut store = PlacementStore::new();
    mount(&mut store, &device, "memo").expect("mount accepted");
    store.reset();

    let evicted = store.reset();

    assert!(evicted.is_empty());
    assert_eq!(store.len(), 0);
}
```

## TC-06 — cap が数えるスロットを全て解放する

| | |
| - | - |
| Setup | `device()`; `MAX_PLACEMENTS` 個の異なる view を mount し、次の mount が拒否される状態にする |
| Act | `let evicted = store.reset();` |
| Expect | `evicted` が `MAX_PLACEMENTS` 個の id を名指す **[CAP]** ／ 後続の mount が受理される **[C3]** |

cap は両スクリーンを合算して数える（`MAX_PLACEMENTS` の doc）。片方のスクリーンしか消さない実装は
テーブルは減っても cap を解放しきれないので、後続の mount 受理まで見て初めて固定できる。

```rust
/// Asserts that a reset frees every slot the placement cap counts.
///
/// Case: a runaway program fills the placement table to its cap, and the
/// user runs `reset` to get the terminal back.
#[test]
fn a_reset_frees_every_slot_the_cap_counts() {
    let device = device();
    let mut store = PlacementStore::new();
    for index in 0..MAX_PLACEMENTS {
        mount(&mut store, &device, &format!("v{index}")).expect("mount fits");
    }
    assert_eq!(mount(&mut store, &device, "overflow"), None);

    let evicted = store.reset();

    assert_eq!(evicted.len(), MAX_PLACEMENTS);
    assert!(mount(&mut store, &device, "after-reset").is_some());
}
```

## TC-A1 — id counter を巻き戻さない（マニュアル由来ではない）

| | |
| - | - |
| Setup | `device()`; `let old = mount(.., "a")`; `store.reset()` |
| Act | `let new = mount(.., "b");` |
| Expect | `new` が `old` より真に大きい **[PI]** |

出典が囲っている型の doc であってマニュアルではないため、他のケースから分離してある。
載せた理由は `*self = Self::new()` という最も自然な実装が `next_id` を巻き戻し、
`PlacementId` の invariant「Ids are minted monotonically per terminal and never reused within a
session, so a delayed id-addressed lifecycle event can never target a successor placement」を
破ること。`ris.md` の決定事項 1 が `next_line_id` について同じ危険を一段下のレイヤで記録している。

名前は `screen/grid.rs` の `mod tests::reset` にある `a_reset_does_not_rewind_the_id_counter`
（`LineId` 側）と衝突しないよう `placement_id` を含めてある。

```rust
/// Asserts that a reset carries the placement id counter forward rather
/// than rewinding it.
///
/// Case: a webview is mounted, the user runs `reset`, and the program
/// mounts a fresh view while the eviction signal for the old one is still
/// in flight.
#[test]
fn a_reset_does_not_rewind_the_placement_id_counter() {
    let device = device();
    let mut store = PlacementStore::new();
    let old = mount(&mut store, &device, "a").expect("mount accepted");
    store.reset();

    let new = mount(&mut store, &device, "b").expect("mount after reset accepted");

    assert!(old < new);
}
```

