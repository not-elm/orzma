# placement の所有権（検討メモ）

**これは確定した設計ではない。** `PlacementStore` を `Screen` へ降ろす案を、実装可能な形まで
落として比較するための作業メモ。採否は未決。

## 何を解こうとしているか

現状は `Vt` が `device: DeviceState` と `placements: PlacementStore` を兄弟に持ち、
`Placement` が `screen: ScreenKind` で自分の所属を記録している。この形が払っているコスト:

| コスト | 位置 |
| - | - |
| `ActiveScreen` 型（grid と kind を組にして取り違えを防ぐためだけに存在） | `device.rs:149-181` |
| `p.screen == active.kind()` の絞り込み 2 箇所 | `placement.rs:84`, `:183` |
| 「非アクティブ側の anchor をアクティブな grid で解決してしまう」バグ用の回帰テスト | `placement.rs:467` |
| reflow の行リマップを「兄弟フィールドなので caller が配送する」必要 | `device.rs:96-101` の TODO |

kitty（`main_grman` / `alt_grman`）、ghostty（`ImageStorage` が `Screen` のフィールド）、
xterm.js（`markers` が `Buffer` のフィールド）はいずれも per-screen 所有で、
「primary は alternate 表示中も隠れるだけ」を構造から無償で得ている。

## 分割線

丸ごと移すことはできない。端末スコープの不変条件が 3 つあるため。

| 何 | どこへ | 根拠 |
| - | - | - |
| anchor / 位置 / 射影 / sweep | **`Screen`** | grid と同じ境界にある |
| `PlacementId` の採番 | **`DeviceState`** | 「per terminal で単調・再利用禁止」（`placement.rs:16-20`） |
| `MAX_PLACEMENTS` の cap | **`DeviceState`** | 「両スクリーン合算 12」。per-screen に置くと最大 24 になる |
| `(view_id, instance_id)` のアドレス空間 | **`DeviceState`** | unmount-all と supersession が両スクリーンに跨る |

ghostty が同じ形を採っている（テーブルは per-screen、識別子の採番だけ上位スコープ）。

## コード骨子

### `Screen` 側

```rust
// screen/placements.rs（新規）
pub(crate) struct ScreenPlacements {
    placements: Vec<Placement>,          // next_id は持たない
}

struct Placement {
    id: PlacementId,
    anchor: LineId,
    col: GridColumn,
    size: PlacementSize,
    view_id: String,
    instance_id: Option<String>,
    // screen: ScreenKind ← 消える。所有者が答えになる
}

/// The cell rectangle a mount reserves, without its position.
pub struct PlacementSize {
    pub rows: u16,
    pub cols: u16,
}
```

`GridSize` と形は同じだが別の型にする。`GridSize` の doc は「row count は
"one screenful" の source of truth」と grid に紐づけており、placement の予約は
grid の寸法ではない。型が分かれていれば取り違えもコンパイルで止まる。

フィールド順は `rows` / `cols`。ワイヤ形式（`mount;<view_id>;<rows>;<cols>`）と
`ApcWebviewVerb::Mount` に合わせる。`GridSize` は `cols` が先だが、そちらに
合わせると placement 側の既存 API と食い違う。

位置を含めないので、**viewport 空間の `ProjectedPlacement` とも共有できる**:

```rust
pub struct ProjectedPlacement {
    pub id: PlacementId,
    pub viewport_row: i32,      // anchor を解決した結果
    pub col: GridColumn,
    pub size: PlacementSize,    // rows / cols を置き換え
}
```

こうすると `ProjectedPlacement` の invariant（"`rows` / `cols` always equal the
mount-time reservation for `id`"）が「projected な `size` は mount 時の `size` に
等しい」と 1 フィールドで言えるようになる。ただし `ProjectedPlacement` は
`orzma_vt::prelude` から export され `orzma_tty_renderer::schema` が再エクスポート
しているので（`schema.rs:12-15`）、共有すると `PlacementSize` も 3 クレートに跨る
公開語彙になる。

`Screen` が所有する。射影と sweep は `&Screen` ではなく解決関数を受ける — でないと
`screen.placements.evict_lost_anchors(&screen)` が可変部分借用と共有借用で衝突する。

```rust
// screen.rs
pub struct Screen {
    grid: Grid,
    viewport: Viewport,
    /* state, scroll_region, tabs, character_set_mapping, checkpoint */
    placements: ScreenPlacements,        // ← 追加
}

impl Screen {
    pub(crate) fn project_placements_into(&self, out: &mut Vec<ProjectedPlacement>) {
        self.placements.project_into(out, |anchor| self.viewport_row_of(anchor));
    }

    /// `self` を分解して借用を割る。
    pub(crate) fn evict_lost_anchors(&mut self) -> Vec<PlacementId> {
        let Self { placements, grid, viewport, .. } = self;
        placements.evict_where(|p| row_of(grid, viewport, p.anchor).is_none())
    }

    /// RIS の screen スコープ。damage に加えて evicted id も返すようになる。
    pub fn reset(&mut self) -> (Option<DamageSpan>, Vec<PlacementId>) { /* .. */ }
}

/// `Screen::viewport_row_of` と sweep が共有する。sweep は `placements` を
/// 可変で握ったまま呼ぶので `&self` メソッドにはできない。
fn row_of(grid: &Grid, viewport: &Viewport, id: LineId) -> Option<i32> { /* .. */ }
```

### `DeviceState` 側

```rust
pub struct DeviceState {
    screens: Screens,
    modes: VtModes,
    colors: Colors,
    placement_ids: PlacementMinter,      // ← 追加（端末に 1 つ）
}

impl DeviceState {
    /// 端末スコープの 3 つをここで満たす。
    pub fn mount_placement(&mut self, rows: u16, cols: u16,
                           view_id: String, instance_id: Option<String>) -> Option<PlacementId> {
        // (a) アドレスは端末単位 — 両スクリーンから supersede
        // (b) cap は両スクリーン合算。supersede の後に見るのは現状どおり
        // (c) id は端末単位で単調
        // → アクティブな Screen へ insert
    }

    /// `?1049` ハンドラ。`PlacementStore::switch_screen` はここへ移る。
    pub fn set_active_screen(&mut self, kind: ScreenKind) -> Vec<PlacementId> {
        self.modes.active_screen = kind;
        match kind {
            ScreenKind::Alternate => Vec::new(),
            ScreenKind::Primary => self.screens.alternate.placements_mut().take_all(),
        }
    }

    /// RIS。両スクリーン分を連結する。
    pub(crate) fn reset(&mut self) -> (Option<DamageSpan>, Vec<PlacementId>) { /* .. */ }
}
```

「primary は alternate 表示中も隠れるだけ」は `set_active_screen` を見れば分かるとおり
**コードが 1 行も要らなくなる**。アクティブな screen の store しか射影しないので、
primary の placement はただそこに在り続ける。

## 消えるもの

| 対象 | 位置 |
| - | - |
| `ActiveScreen` 型ごと（4 メソッド + 30 行） | `device.rs:149-181` |
| `DeviceState::active_screen()` | `device.rs:80-85` |
| `Placement.screen: ScreenKind` | `placement.rs:235` |
| kind 絞り込み 2 箇所 | `placement.rs:84`, `:183` |
| `switch_screen` の `evict_where(\|p\| p.screen != Primary)` | `placement.rs:198` |
| 回帰テスト `a_sweep_leaves_the_other_screens_placement_alone` | `placement.rs:467` — 守る失敗モードが存在しなくなる |
| `FrameTracker::emit` の第 2 引数 | `frame.rs:143` → `emit(&mut self, device: &DeviceState)` |

## 代償

`Screen::reset`（および将来 scroll と同時に sweep するなら `line_feed` 系）が damage に加えて
`Vec<PlacementId>` を返すようになる。現状の `Screen` は damage だけを返し、signal と liveness は
`Executor` の責務、という境界が広がる。

回避案として evicted id を `Screen` 内にバッファして `Executor` が `take_evicted()` で
drain する形もあるが、「変更は返り値で表す」という今の作りから外れる。

## 移行コスト

`mount` / `unmount` / `switch_screen` / `evict_lost_anchors` を呼ぶ **production コードはまだ 1 行も無い**
（`apc_dispatch` と `?1049` ハンドラが `todo!()`）。影響 34 箇所のうち production は `frame.rs` 3 行と
`lib.rs:264` だけで、残りはテスト。APC と `?1049` を結線した後にやるとこの比率は悪化する。

## 未決事項

1. **採否そのもの。** 現状維持でも RIS（[ris.md](ris.md)）は進められる。
   [tdd-placement-reset.md](tdd-placement-reset.md) の 7 ケースのうち構造に依存するのは TC-03 だけで、
   それも「両 `Screen` の store を連結する」形に変わるだけで消えない。
2. **着手タイミング。** 上記のとおり今が最も安いが、RIS と同時にやると関心が混ざる。
3. **`Screen::reset` の戻り値の形。** 組を返すか `take_evicted()` にするか。
4. **`PlacementSize` を `ProjectedPlacement` と共有するか。** 共有すると 3 クレートに跨る
   公開語彙になり、`orzma_webview/src/webview/mount.rs:56` の doc も追随が要る。
   共有しないなら `Placement` 内部だけの整理で、`pub` も不要。

## 検討済みで採らなかった案

| 案 | 却下理由 |
| - | - |
| `ActiveScreen` を `&Screen` に置換 | `kind()` の呼び出し元が 3 箇所ある。placement を降ろさない限り消せない |
| `&Screen` と `ScreenKind` の 2 引数 | `ActiveScreen` の doc が unconstructible と言う取り違えを再導入する |
| `Screen` に `kind` フィールドを追加 | `VtModes::active_screen` の "the only record" と `Screens` の "pure storage" に衝突 |
| `instance_id` を VT が採番して `PlacementId` を廃止 | supersession が死に、再 mount のたびに host の entity が despawn → ページリロード。加えて `mount` が fire-and-forget でなくなる。`PlacementId` はクライアントプロトコルに露出していない（`orzma_webview_protocol.md` に 0 件）ので、内部の重複 1 個のためにプロトコルの性質を変える交換になる |
