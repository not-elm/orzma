## テストケース一覧

| # | 名前 | Source | 優先度 | 実装先 |
| - | - | - | - | - |
| TC-01 | `a_reset_leaves_this_screens_placements_unresolvable` | C3 + LC | High | 実装済み。`screen.rs` の `mod placements`（Task 3） |
| TC-02 | `a_reset_drops_a_placement_the_sweep_alone_would_leave` | C3 + LC | High | 実装済み。`screen.rs` の `mod placements` |
| TC-03 | — | C3 + LC | High | `a_sweep_reaches_the_inactive_screen`（`device.rs`、Task 4）に統合済み。単独のテストとしては存在しない |
| TC-04 | `a_reset_still_names_a_placement_whose_anchor_already_left_the_grid` | C3 + LC | High | 未実装。`screen.rs` の `mod placements` へ追加 |
| TC-05 | `a_second_reset_reports_no_further_eviction` | C3 + LC | Medium | 未実装。`screen.rs` の `mod placements` へ追加 |
| TC-06 | `a_mount_is_accepted_once_the_sweep_frees_the_cap` | CAP + C3 | Medium | 未実装。`device.rs` の `mod tests` へ追加 |
| TC-A1 | `a_reset_does_not_rewind_the_device_placement_id_counter` | PI | High（実装上の罠） | 実装済み。`device.rs` の `mod tests` |

Source タグ — **C3**: ECMA-48 p.69 L3214（初期状態への復帰）／**LC**: `VtSignal::WebviewEvicted` と
`evict_lost_anchors` の doc が定めるライフサイクル契約／**CAP**: `MAX_PLACEMENTS` の doc／
**PI**: `PlacementId` の invariant。

TC-01 から TC-06 までが仕様契約 C3 とそのライフサイクル契約に対応し、TC-A1 だけがマニュアル由来ではない。

対象は `PlacementStore::reset()` の単発呼び出しから、`Screen::reset()`（RIS のスクリーン側の効果）
と `evict_lost_anchors()`（チャンク末尾の sweep）の 2 段へ変わった。`PlacementStore` は削除済みで、
`Screen` が自分の表を、`DeviceState` が id 採番・cap・アドレス空間を持つ（設計は
[placement-ownership-design.md](../superpowers/specs/2026-08-26-placement-ownership-design.md)）。
`DeviceState::reset()`（[ris.md](ris.md) #3）は実装済みなので、cap や id 採番など端末スコープの
契約が要るケースはそれを、単一スクリーンで足りるケースは `Screen::reset()` を直接呼ぶ。前者は
`device.rs` の `mod tests`（`fn device()` / `fn mount()` ヘルパを使う）へ、後者は `screen.rs` の
`mod placements`（`fn screen()` / `fn mount()` ヘルパを使う）へ置く。

## TC-01 — 全 placement を破棄し、その id を名指す

実装済み。`screen.rs` の `a_reset_leaves_this_screens_placements_unresolvable` がこの契約を
`Screen::reset()` → `Screen::evict_lost_anchors()` の 2 段で固定している。

```rust
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
```

返り値の順序はここでは id の採番順と一致するので、ソートせず直接比較できる。順序そのものを
契約するのは `DeviceState::evict_lost_anchors`（primary が先）の側であって、このテストではない。

## TC-02 — sweep 単独では残る anchor を reset が落とす

| | |
| - | - |
| Setup | `screen()`; カーソルを最終行へ; `mount(&mut screen, 1, "memo")`; `line_feed()` を 3 回で anchor を履歴へ押し出す |
| Act | 押し出し直後に `project_placements()` と `evict_lost_anchors()` を確認してから `screen.reset()`、さらに `evict_lost_anchors()` |
| Expect | reset 前は `project_placements()` が解決し `evict_lost_anchors()` は何も落とさない **[LC]**。reset 後は `evict_lost_anchors()` が id を名指す **[C3]** |

anchor は grid 空間の負の行（`GridLine(-1)`）に解決される。viewport の外だが grid のリングには
まだ在るので、sweep 単独ではまだ evictable ではない。

`line_feed()` が 3 回でなければならない理由: mount 時点で anchor は screen line 2、履歴は 0 行なので
`GridLine(2)`。カーソルは既に最終行にいるので line feed のたびに `scroll_up_one(0, 2)` が走り、
履歴が 1 行ずつ伸びて anchor の `GridLine` が 1 ずつ下がる（2 → 1 → 0 → -1）。1 回では
`GridLine(1)` にしかならない。あわせて履歴が伸びるため `Grid::is_blank()` は false になり、
`Screen::reset()` は `None` ではなく `Some(DamageSpan::Full)` を返す。

元の TC-02 は「reset が sweep の述語（当時の `viewport_row_of(..).is_none()`）をそのまま流用する
実装」を禁じるために書かれていた。理由は、viewport の外だが grid には生きている anchor をその
述語だけで判定すると、reset 直後もまだ解決できてしまい誤って生存させる、という懸念だった。
本設計はまさにその流用を採る: `Screen::reset` は placement を直接触らず、`evict_lost_anchors`
（sweep）と全く同じ式 `|anchor| self.grid.grid_line(anchor)` に判定を委ねている。これが健全なのは
`Grid::reset` が reset の時点で全行を作り直し id を振り直しているからで、sweep が動く頃には reset
前のどの anchor も同じ述語が `None` を返す。禁止が消えたのではなく、禁止が成立する前提
（`Grid::reset` が先に anchor を破壊する）が別の理由で満たされるようになった、という順序への
依存が根拠であり、これを書き残さないと将来の読み手が「述語の流用は禁止のはず」という旧い
結論を史料無しに再導出してしまう。

```rust
/// Asserts that an anchor pushed into history projects to a negative
/// grid line and survives the sweep, and that a reset is what makes
/// that same placement evictable.
///
/// Case: a webview was mounted beside a line of output that the shell
/// has since scrolled into the scrollback, and the shell then sends
/// `ESC c`.
#[test]
fn a_reset_drops_a_placement_the_sweep_alone_would_leave() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    mount(&mut screen, 1, "memo");
    for _ in 0..3 {
        screen.line_feed();
    }
    assert_eq!(screen.project_placements()[0].point.line, GridLine(-1));
    assert!(screen.evict_lost_anchors().is_empty());

    assert_eq!(screen.reset(), Some(DamageSpan::Full));

    assert_eq!(screen.evict_lost_anchors(), vec![PlacementId(1)]);
}
```

## TC-03 — 非アクティブスクリーンの placement

この設計の要だが、単独のテストとしては置かない。`device.rs` の `a_sweep_reaches_the_inactive_screen`
（Task 4）が同じ契約を固定している。両スクリーンを走査する `DeviceState::evict_lost_anchors` の
実装そのものが、`switch_screen` の「Primary の placement は alternate 表示中も破棄せず隠すだけ」
という前例との差分を体現している。

## TC-04 — anchor が既に grid buffer から消えている placement

| | |
| - | - |
| Setup | `screen()`; カーソルを最終行へ; `mount(&mut screen, 1, "memo")`; `reverse_index()` を 3 回で anchor をリングから追い出す |
| Act | `screen.reset()`、続けて `evict_lost_anchors()` |
| Expect | `evicted` が当該 id を名指す **[LC]** ／ テーブルが空 **[C3]** |

このケースは reset の前から `project_placements()` が空、つまり anchor は reset の有無に関わらず
落ちる。`screen.rs` の `a_placement_the_projection_omits_is_also_swept` が全く同じ setup を reset を
挟まずに固定済みなので、新設はその間に `reset()` を挟んでも壊れないことの確認に留まり、新しい
生死判定を足すものではない。

```rust
/// Asserts that a reset still names a placement whose anchor already
/// left the grid before the reset ran.
///
/// Case: a webview's anchor row was recycled by a reverse scroll and no
/// eviction sweep has run yet when `ESC c` arrives.
#[test]
fn a_reset_still_names_a_placement_whose_anchor_already_left_the_grid() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    mount(&mut screen, 1, "memo");
    for _ in 0..3 {
        screen.reverse_index();
    }
    assert!(screen.project_placements().is_empty());

    assert_eq!(screen.reset(), None);

    assert_eq!(screen.evict_lost_anchors(), vec![PlacementId(1)]);
}
```

## TC-05 — 2 度目の reset は何も報告しない

| | |
| - | - |
| Setup | `screen()`; `mount(&mut screen, 1, "memo")`; `screen.reset()`; `evict_lost_anchors()` で 1 回目を drain |
| Act | `screen.reset()`、続けて `evict_lost_anchors()` |
| Expect | `evicted` が空 **[LC]** ／ テーブルが空 **[C3]** |

すでに空になったテーブルに対する reset のケースと setup 以外が同一なのでマージした。

```rust
/// Asserts that a reset of an already-empty table reports no further
/// eviction.
///
/// Case: the user runs `reset` twice in a row at a fresh prompt.
#[test]
fn a_second_reset_reports_no_further_eviction() {
    let mut screen = screen();
    mount(&mut screen, 1, "memo");
    assert_eq!(screen.reset(), None);
    assert_eq!(screen.evict_lost_anchors(), vec![PlacementId(1)]);

    assert_eq!(screen.reset(), None);
    assert!(screen.evict_lost_anchors().is_empty());
}
```

## TC-06 — sweep の後に mount が受理される

| | |
| - | - |
| Setup | `device()`; `MAX_PLACEMENTS` 個の異なる view を mount し、次の mount が拒否される状態にする |
| Act | `device.reset()` を呼び、続けて `device.evict_lost_anchors()` を呼んでから再度 mount する |
| Expect | `evict_lost_anchors()` が `MAX_PLACEMENTS` 個の id を名指す **[CAP]** ／ その後の mount が受理される **[C3]** |

cap は両スクリーンを合算して数える（`MAX_PLACEMENTS` の doc）。cap の解放はチャンク末尾の sweep
まで遅延するので（[placement-ownership-design.md](../superpowers/specs/2026-08-26-placement-ownership-design.md)
の「この一本化の代償 (c)」）、`device.reset()` を呼んだだけではまだ解放されておらず、`evict_lost_anchors()`
を挟んで初めて後続の mount が通ることを確かめる。

```rust
/// Asserts that a mount is accepted once the sweep following a reset
/// frees the cap it counts across both screens.
///
/// Case: a runaway program fills the placement table to its cap, the
/// user runs `reset` to get the terminal back, and the shell then
/// starts a fresh webview.
#[test]
fn a_mount_is_accepted_once_the_sweep_frees_the_cap() {
    let mut device = device();
    for index in 0..MAX_PLACEMENTS {
        mount(&mut device, &format!("v{index}")).expect("a mount under the cap is accepted");
    }
    assert!(mount(&mut device, "overflow").is_none());

    assert_eq!(device.reset(), None);
    assert_eq!(device.evict_lost_anchors().len(), MAX_PLACEMENTS);

    assert!(mount(&mut device, "after-reset").is_some());
}
```

## TC-A1 — id counter を巻き戻さない（マニュアル由来ではない）

出典が囲っている型の doc であってマニュアルではないため、他のケースから分離してある。対象は
`Screen` の行 id ではなく `DeviceState::next_placement_id`（端末スコープの placement id 採番）に
変わった。`Screen::reset()` はこのカウンタに一切触れない — id 採番は `DeviceState` の責務であって
`Screen` の 8 フィールドのどれにも属さないため、reset の対象にそもそも入っていない。
[ris.md](ris.md) #3（`DeviceState::reset()`）が実装された今、このテストが守っているのは、その実装が
`*self = Self::new()` のような素朴な形を取って `next_placement_id` を巻き戻さないことそのものである。
素朴な形へ書き換えるとこのテストだけが落ちることを確認してある。

`PlacementId` の invariant「Ids are minted monotonically per terminal and never reused within a
session, so a delayed id-addressed lifecycle event can never target a successor placement」を
破ると、host 側で古い id 宛てのライフサイクルイベントが新しい placement を誤って指してしまう。
[ris.md](ris.md) の決定事項 1 が `next_line_id` について同じ危険を一段下のレイヤで記録している。

名前は `screen/grid.rs` の `mod tests::reset` にある `a_reset_does_not_rewind_the_id_counter`
（`LineId` 側）と衝突しないよう `device_placement_id` を含めてある。

```rust
/// Asserts that a reset does not rewind the device's placement id
/// counter.
///
/// Case: a webview is mounted, the user runs `reset`, and the program
/// mounts a fresh view while the eviction signal for the old one is
/// still in flight.
#[test]
fn a_reset_does_not_rewind_the_device_placement_id_counter() {
    let mut device = device();
    let old = mount(&mut device, "a").expect("mount accepted");
    assert_eq!(device.reset(), None);
    device.evict_lost_anchors();

    let new = mount(&mut device, "b").expect("mount after reset accepted");

    assert!(old < new);
}
```
