# placement の所有権を `Screen` へ降ろす

> `docs/superpowers/` は `.gitignore:9` で無視されるが、本書は `git add -f` で例外的に
> 追跡している。tracked な `docs/todo/` から参照できるようにするため。

`PlacementStore` を分割し、表そのものを `Screen` が所有する形へ移す。端末スコープの
不変条件（id 採番・cap・アドレス空間）は `DeviceState` が引き取る。あわせて eviction
の報告経路を「チャンク末尾の両スクリーン sweep」に一本化し、`Screen::reset` が
placement について何も返さない形にする。

検討の経緯は [placement-ownership.md](../../todo/placement-ownership.md) にある。本書は
その検討メモを実装可能な仕様まで落としたもので、メモの内容とは以下が異なる。

- 表の置き場所を `screen/placements.rs` に確定した
- メモの `row_of(grid, viewport, id)` 自由関数を廃した。フレームが運ぶ placement 座標を
  viewport 空間から grid 空間へ移した（`ProjectedPlacement` → `AnchoredPlacement`）ので、
  VT 側にオフセット適用そのものが要らなくなった
- eviction を sweep に一本化したため、メモの未決 #3（`Screen::reset` の戻り値）が問題ごと消えた
- sweep をアクティブスクリーン限定から両スクリーンへ広げた

## スコープ

含む:

- `PlacementStore` の解体と `ScreenPlacements` / `DeviceState` への再配置
- `ActiveScreen` 型の削除
- `ProjectedPlacement` の grid 空間化（`AnchoredPlacement`）と、消費側 `orzma_webview` の追随
- sweep のアクティブスクリーン限定を解除し、両スクリーンを走査する形へ
- `FrameTracker` と `Executor` の借用構成の追随
- 上記に伴うテストの移設と追加

含まない（別案件）:

- `Executor::esc_dispatch` の RIS アーム（`ris.md` #5）
- `apc_dispatch` の webview verb 結線
- `?1049` ハンドラの結線
- reflow 付き `DeviceState::resize` と、それに伴う anchor の再マッピング

含まない項目はいずれも現在 `todo!()` であり、本設計は「結線したときに何を呼ぶか」を
確定させるところまでを担当する。

## 何を解いているか

現状は `OrzmaVt` が `device: DeviceState` と `placements: PlacementStore` を兄弟に持ち、
`Placement` が `screen: ScreenKind` で自分の所属を記録している。この形が払っているコスト:

| コスト | 位置 |
| - | - |
| `ActiveScreen` 型（grid と kind を組にして取り違えを防ぐためだけに存在） | `device.rs:149-181` |
| `p.screen == active.kind()` の絞り込み 2 箇所 | `placement.rs:93`, `:181` |
| 「非アクティブ側の anchor をアクティブな grid で解決してしまう」バグ用の回帰テスト | `placement.rs:467` |
| reflow の行リマップを「兄弟フィールドなので caller が配送する」必要 | `device.rs:96-101` の TODO |
| `Executor` の借用フィールド 6 個のうち 1 個 | `interpreter.rs:101` |

kitty（`main_grman` / `alt_grman`）、ghostty（`ImageStorage` が `Screen` のフィールド）、
xterm.js（`markers` が `Buffer` のフィールド）はいずれも per-screen 所有で、
「primary は alternate 表示中も隠れるだけ」を構造から無償で得ている。

## 分割線

丸ごと移すことはできない。端末スコープの不変条件が 3 つあるため。

| 何 | どこへ | 根拠 |
| - | - | - |
| anchor / 位置 / 射影 / sweep | **`Screen`** | grid と同じ境界にある |
| `PlacementId` の採番 | **`DeviceState`** | 「per terminal で単調・再利用禁止」（`placement.rs` の `PlacementId` invariant） |
| `MAX_PLACEMENTS` の cap | **`DeviceState`** | 両スクリーン合算。理由は下記 |
| `(view_id, instance_id)` のアドレス空間 | **`DeviceState`** | unmount-all と supersession が両スクリーンに跨る |

ghostty が同じ形を採っている（テーブルは per-screen、識別子の採番だけ上位スコープ）。

### cap が per-screen になれない理由

`MAX_PLACEMENTS = 12` はレンダラの `OVERLAY_SLOTS = 12` の写しで、「VT が受理した mount は
host が必ず置ける」を保証している。per-screen 12 にすると合計 24 になり、この保証が破れる。

破れ方は次のとおり。host のスロットは**描画中の数ではなく生存している子の数**で埋まる
（`orzma_webview/src/webview/mount.rs:497` の `smallest_free_slot` が数えるのは
`live_webview_children(.., terminal_surface)`、つまり当該 terminal の webview 子エンティティ全部）。
そして非アクティブスクリーンの placement は破棄されず隠れるだけなので、隠れた 12 個も
スロットを握り続ける。したがって primary に 12 個 mount → alternate へフリップ → もう 1 個
mount、という手順で host 側のスロットが枯渇し、`mount.rs:250` で **debug ログ 1 行を出して
黙って捨てられる**。VT の表には id を持つ placement が残り、フレームの射影リストにも載るが、
**対応する子エンティティが存在しないので `project_webview_overlays` のループはそれを一度も
訪問しない**（`mount.rs:592-602` は webview の子を回るのであって射影リストを回るのではない。
`:601` の `continue` は逆のケース、つまり子はあるが id がリストに無いときの処理）。
結果として永久に描画されず、エラーも上がらない。

スロット数を増やす方向も塞がっている。**効いているのはバインディングの形の方**で、
WGSL はテクスチャを動的インデックスできないため、オーバーレイは 12 個の個別 binding として
宣言され（`terminal_ui_material.wgsl:70-81`）、サンプリングも手書きでアンロールされている
（`:447-458`）。増やすにはシェーダのソースと `bind_group_layout_entries` の両方を書き換えることになる。

数値上限もその先にある。sampled texture の消費はアトラス 1 + オーバーレイ 12 = 13 で、
`wgpu::Limits::default()` の `max_sampled_textures_per_shader_stage: 16` に対する余裕は 3。
つまり上限だけを見れば 15 までは許されるので、**24 を塞いでいるのは上限だが、12 を選ばせて
いるのは上限ではない**。24 にすると 25 で超える。

なお `OVERLAY_SLOTS` の doc が引く「spec §6.1」はこのリポジトリに存在せず（`docs/` 全文検索で 0 件）、
12 という具体的な数の一次資料は失われている。上記は現行コードから読み取れる制約であって、
当時の判断根拠そのものではない。

## 設計

### `placement.rs` — 公開語彙と cap だけが残る

```rust
pub struct PlacementId(pub u64);
pub struct PlacementSize { pub rows: u16, pub cols: u16 }

/// One placement's grid-space geometry at emit time.
pub struct AnchoredPlacement {
    pub id: PlacementId,
    pub point: GridPoint,
    pub size: PlacementSize,
}

/// The most placements one terminal may hold across both screens.
pub const MAX_PLACEMENTS: usize = 12;
```

`PlacementStore` と `Placement` は消える。`MAX_PLACEMENTS` は無修飾から `pub` へ広がる
（`device.rs` が cap を執行するため）。3 つの型は `orzma_vt::prelude` から
`orzma_tty_renderer::schema` へ再エクスポートされる現行の経路をそのまま維持する
（`schema.rs:12-16` の `ProjectedPlacement` を `AnchoredPlacement` へ差し替える。`GridPoint` は
既に再エクスポート済み）。

`ProjectedPlacement` からの改名は座標空間の変更を伴う。次節がその根拠を持つ。

### 座標空間 — placement は grid 空間で出る

`ProjectedPlacement.viewport_row: i32` は、フレームが運ぶ「アンカーされた内容」のうち唯一
viewport 空間で出ていた。他はすべて grid 空間で、射影は消費側が行う。

| 種別 | 空間 | 射影する側 |
| - | - | - |
| `DirtyRow.line: ViewportLine` | viewport | ——（リスト自体が再描画対象の viewport 行） |
| `Cursor.point: GridPoint` | grid | 消費側（`orzma_tty_renderer/src/schema/grid.rs:127`） |
| `ViCursor` | grid | 消費側（`schema/grid.rs:138`） |
| `SelectionRange.start` / `.end` | grid | 消費側 |
| `ProjectedPlacement.viewport_row` | **viewport** | **VT** |

`SelectionRange` の doc（`selection.rs:11-14`）がこの規約を明文化している。

> The endpoints are raw grid positions and do not move when the user scrolls; project them with
> `GridLine::to_viewport` to place the highlight on screen.

`AnchoredPlacement { id, point: GridPoint, size }` は placement をこの規約に戻す。
`viewport_row: i32` と `col: GridColumn` は `GridPoint` に畳まれ、`Cursor` や `SelectionRange` と
同じ形になる。フレームの契約から raw な `i32` 座標が消える。

消費側の追随は 1 箇所で足りる。`project_webview_overlays`（`mount.rs:572`）は `&TerminalGrid` を
取り、そこに `display_offset` が載っており（`schema/grid.rs:56`）、`Changed` ゲート無しで毎フレーム
全 placement の rect を組み直している。既に `let row = i64::from(projected.viewport_row);` と `i64` へ
昇格してカリング判定しているので、`i64::from(point.line.0) + i64::from(grid.display_offset)` に
替わるだけになる。カリング（`row + size.rows <= 0 || row >= grid.rows`）は元から消費側にある。

**`GridLine` に非カリングの射影ヘルパは足さない。** `GridLine::to_viewport` は `0..rows` の外を
`None` にするので placement には使えない（上端がはみ出していても本体は描く）が、その隣に `-> i32`
を返す 2 本目の変換を置くと `GridLine` の変換口が 2 つになり、`ViewportLine` との区別が曖昧になる。
本設計の後、符号付きの行を計算するのは `orzma_webview` の 1 箇所だけであり、逆方向
（viewport → grid）は既に `orzma_tty_renderer/src/grid.rs:44`, `:86` でベタ書きされている。
2 箇所目が現れたら、`display_offset` を所有する `TerminalGrid` の側に置く。

副次的に、`FrameTracker::diff_placements`（`frame.rs:186-193`）が**ユーザースクロールで発火しなく
なる**。現行は viewport を 1 行動かすだけで全 `viewport_row` がずれ、リスト全体が再送される。
grid 空間ではユーザースクロールに対して不変なので `placements: None` に落ちる。内容のスクロールでは
依然として全件変わる —— `GridLine` は frame-local で、履歴へ押し出された行があると既存テキストの
line が全て -1 されるため（`coords.rs:13-15`）。

あわせて `ViewportLine` の doc（`screen/viewport.rs:21-25`）から、`u16` では表現できない負値と
`-1` センチネルの記述を落とす。あの規約は `orzma_tty_engine` のワイヤ型（`ViCursor.row: i16` /
`ViewportPoint.row: i16`、`frame_builder.rs:176-186`, `:222-223`）のもので、`orzma_vt` の
`ViewportLine` は `Screen::viewport_row` の添字・`DamageSpan::rows`・`DirtyRow.line` にしか
使われず、常に非負かつ範囲内である。

### 可視性の書き方

**メソッドと関連定数は `pub`、`pub(crate)` は使わない。** `mod screen;` も `mod device;` も
`mod placement;` も `lib.rs` で無修飾（`lib.rs:19-26`）なので、モジュールツリーの時点で
クレート外に出ない。型の側で既に絞ってあるものに `pub(crate) fn` を重ねても実効可視性は
変わらず、宣言が長くなるだけになる。既存の `PlacementStore`（`pub(crate) struct`）も
`DeviceState`（同）も、メソッドは全て `pub fn` で書かれている。

現行で唯一 `pub(crate) fn` になっている `Screen::viewport_row_of`（`screen.rs:614`）は本設計で
消えるので（後述）、例外は残らない。

### `screen/placements.rs`（新規）— 1 スクリーン分の表

`Screen` のフィールドの型は例外なく `screen/` 配下にある（`grid` / `viewport` / `state` /
`scroll_region` / `tabs` / `character_set_mapping` / `checkpoint` の 7 つ全て）。8 番目も
その慣習に従う。

```rust
//! The webview placements one screen owns: the mount table, the
//! resolution of its anchors into grid coordinates, and the eviction
//! sweep.

/// The placements mounted on one screen.
///
/// Which screen owns the table answers which screen a placement belongs
/// to, so no placement records its own screen.
pub(crate) struct ScreenPlacements {
    placements: Vec<Placement>,
}

impl ScreenPlacements {
    pub fn new() -> Self;

    /// Number of placements this screen holds.
    pub fn len(&self) -> usize;

    /// Whether this screen holds no placement.
    ///
    /// [`Self::evict_lost_anchors`] returns on this before resolving a
    /// single anchor, which is what keeps sweeping a screen with no
    /// webview free.
    pub fn is_empty(&self) -> bool;

    /// Registers a mount; the caller has already minted `id` and
    /// resolved the anchor.
    pub fn mount(
        &mut self,
        id: PlacementId,
        anchor: LineId,
        col: GridColumn,
        size: PlacementSize,
        view_id: String,
        instance_id: Option<String>,
    );

    /// Drops the placement a re-mount replaces, without reporting it.
    ///
    /// A superseded id is deliberately unnamed: the host re-points the
    /// same entity at the successor and would despawn it if the
    /// superseded id were reported as evicted.
    pub fn supersede(&mut self, view_id: &str, instance_id: Option<&str>);

    /// Removes the placements a client `unmount` addresses; returns
    /// whether anything went.
    pub fn unmount(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool;

    /// Resolves every placement's anchor through `line_of` — the
    /// complete list, not a diff.
    pub fn project(
        &self,
        line_of: impl Fn(LineId) -> Option<GridLine>,
    ) -> Vec<AnchoredPlacement>;

    /// Drops the placements `line_of` can no longer resolve and names them.
    ///
    /// # Invariants
    ///
    /// `line_of` is the same resolver [`Self::project`] takes. A placement
    /// this rejects is exactly a placement projection would omit, so no
    /// placement can become unresolvable without also becoming evictable.
    pub fn evict_lost_anchors(
        &mut self,
        line_of: impl Fn(LineId) -> Option<GridLine>,
    ) -> Vec<PlacementId>;

    /// Empties the table and names every id it held.
    pub fn take_all(&mut self) -> Vec<PlacementId>;

    fn evict_where(&mut self, should_evict: impl FnMut(&mut Placement) -> bool) -> Vec<PlacementId>;
}

/// One mounted webview on this screen.
struct Placement {
    id: PlacementId,
    anchor: LineId,
    col: GridColumn,
    size: PlacementSize,
    view_id: String,
    instance_id: Option<String>,
}
```

`Placement` とそのフィールドはこのファイルに閉じる。`Screen` 側が `Placement` を組み立てる
必要が無いよう、`mount` が構成要素を受け取る形にした。

anchor の解決をクロージャで受けるのは、`ScreenPlacements` が `Grid` を知らずに済ませるため。
`project` と `evict_lost_anchors` は同じ `impl Fn(LineId) -> Option<GridLine>` を受ける。

生死判定は「射影が省く placement」と同義でなければならない。2 本の式に分けると、片方だけが
将来変わったときに**射影されないのに sweep もされない** placement が生まれ、cap スロットと
host 側の webview 子エンティティを永久に占有したまま、どこにもエラーが出ない。

**シグネチャを揃えるだけでは同義性は保証されない。** 同じ境界を満たす独立した 2 つのクロージャを
書くことを Rust は禁じないので、型は乖離を止めない。ここでは両者に**同一の式**
`|anchor| self.grid.grid_line(anchor)` を渡すことで解く。生死を決めているのは `Grid::grid_line` が返す
`Option` ただ 1 つで、その手前にも後ろにも `None` を作りうる段が無い。placement を grid 空間で出す
（前節）ことで VT からオフセット適用が消え、この形が可能になった。

### `Screen` — 14 番目の impl ブロック

```rust
pub struct Screen {
    grid: Grid,
    viewport: Viewport,
    state: ScreenState,
    scroll_region: ScrollRegion,
    tabs: TabStops,
    character_set_mapping: CharacterSetMapping,
    checkpoint: Checkpoint,
    placements: ScreenPlacements,   // 8 番目
}

/// Webview placements.
///
/// The anchor a mount records is a `LineId` from this screen's own grid,
/// so a placement can only ever be resolved against the grid that minted
/// its anchor.
///
/// Four of these forward to [`ScreenPlacements`] unchanged. They stay
/// rather than exposing the table, so `DeviceState` never holds a
/// `&mut ScreenPlacements` and every mutation of a screen's placements
/// goes through the screen that owns them.
impl Screen {
    /// Registers a mount at the write cursor under an already-minted id.
    pub fn mount_placement(
        &mut self,
        id: PlacementId,
        size: PlacementSize,
        view_id: String,
        instance_id: Option<String>,
    ) {
        let anchor = self.cursor_line_id();
        let col = self.cursor_column();
        self.placements.mount(id, anchor, col, size, view_id, instance_id);
    }

    pub fn supersede_placement(&mut self, view_id: &str, instance_id: Option<&str>);
    pub fn unmount_placement(&mut self, view_id: Option<&str>, instance_id: Option<&str>) -> bool;
    pub fn take_placements(&mut self) -> Vec<PlacementId>;
    pub fn placement_count(&self) -> usize;

    /// Resolves this screen's placements into grid coordinates.
    ///
    /// # Invariants
    ///
    /// The anchors resolve through the same expression
    /// [`Self::evict_lost_anchors`] passes. A placement this omits is
    /// exactly a placement the sweep evicts, so no placement can become
    /// unresolvable without also becoming evictable.
    pub fn project_placements(&self) -> Vec<AnchoredPlacement> {
        self.placements.project(|anchor| self.grid.grid_line(anchor))
    }

    /// Drops the placements whose anchor row left this screen's grid and
    /// names them.
    pub fn evict_lost_anchors(&mut self) -> Vec<PlacementId> {
        self.placements.evict_lost_anchors(|anchor| self.grid.grid_line(anchor))
    }
}
```

既存の `Screen::viewport_row_of`（10 番目のブロック、`screen.rs:614`）と、その委譲先である
`ActiveScreen::viewport_row_of`（`device.rs:173`）は**消える**。呼び出し元は `placement.rs:97` と
`:181` の 2 箇所しかなく、どちらも `Grid::grid_line` の直呼びに変わる。

借用について。**フィールド分割（`let Self { placements, grid, .. } = self;`）は要らない。**
sweep はレシーバに `&mut self.placements` を取りながらクロージャが `self.grid` を共有捕捉するが、
RFC 2229 の精密キャプチャで捕捉されるのは `self.grid` という place だけで `*self` 全体ではないため、
2 つの借用は disjoint として通る。

これは private リゾルバを持たない形にした結果である。`&self` レシーバのメソッドを述語に渡す形
（`|a| self.viewport_row_of(a)`）はクロージャが `*self` 全体を捕捉して `&mut self.placements` と
衝突し、**E0502 で通らない** —— 精密キャプチャが place を絞れるのはフィールドを直接読むときだけで、
メソッド呼び出しでは絞れない。当初案が private リゾルバに 2 フィールドを渡していたのはこの制約を
避けるためだったが、フィールドを直に叩く今の形では制約そのものが現れない。

以上 2 点は 1.95 / edition 2024 で最小再現をコンパイルして確認した。

**`ScreenPlacements` と `Placement` には `#[derive(Debug)]` が要る。** `Screen` が
`#[derive(Debug)]` を持つ（`screen.rs:62`）ので、derive できないフィールドを足すとその derive が壊れる。

### `DeviceState` — 端末スコープの 3 つ

```rust
pub(crate) struct DeviceState {
    screens: Screens,
    modes: VtModes,
    colors: ColorTable,
    title: TitleState,
    next_placement_id: PlacementId,   // 追加
}

/// Webview placements.
///
/// The three terminal-scoped invariants live here because none of them
/// can be satisfied by one screen alone: ids are minted per terminal,
/// the cap counts both screens, and a `(view_id, instance_id)` address
/// is unique across the pair.
impl DeviceState {
    /// Registers a mount at the active screen's cursor and mints its id;
    /// `None` when the cap rejects it.
    ///
    /// # Invariants
    ///
    /// Supersession runs before the cap check: a re-mount frees the slot
    /// it takes, so it must succeed even at the limit.
    pub fn mount_placement(
        &mut self,
        size: PlacementSize,
        view_id: String,
        instance_id: Option<String>,
    ) -> Option<PlacementId> {
        self.supersede_placement(&view_id, instance_id.as_deref());
        if MAX_PLACEMENTS <= self.placement_count() {
            return None;
        }
        let id = self.mint_placement_id();
        self.active_mut().mount_placement(id, size, view_id, instance_id);
        Some(id)
    }

    /// Removes the placements a client `unmount` addresses on either
    /// screen; returns whether anything went.
    ///
    /// # Invariants
    ///
    /// Every screen is visited — the accumulation must not short-circuit.
    /// A broad scope (`view_id` alone, or unmount-all) can match on both
    /// screens, and the host despawns every matching child across the
    /// terminal in one pass, so a VT that stopped at the first match
    /// would keep a placement holding a cap slot whose host child is
    /// already gone.
    pub fn unmount_placement(
        &mut self,
        view_id: Option<&str>,
        instance_id: Option<&str>,
    ) -> bool {
        let primary = self.screens.primary.unmount_placement(view_id, instance_id);
        let alternate = self.screens.alternate.unmount_placement(view_id, instance_id);
        primary || alternate
    }

    /// Sweeps both screens for placements whose anchor no longer resolves
    /// and names them.
    ///
    /// # Invariants
    ///
    /// The primary screen's ids come first. The order is observable —
    /// the host acts on the returned list in sequence — and this is the
    /// only operation that exposes it, so it is fixed here.
    pub fn evict_lost_anchors(&mut self) -> Vec<PlacementId>;

    /// Applies an alternate-screen flip, tearing down the placements the
    /// abandoned alternate screen owned.
    pub fn switch_screen(&mut self, to: ScreenKind) -> Vec<PlacementId>;

    /// Drops the placement a re-mount replaces on either screen.
    fn supersede_placement(&mut self, view_id: &str, instance_id: Option<&str>);

    /// Live placements across both screens — what the cap counts.
    fn placement_count(&self) -> usize;

    /// Hands out the next unused placement id.
    ///
    /// # Invariants
    ///
    /// Ids only ever move forward, which is what lets a delayed
    /// id-addressed lifecycle event be matched against a placement that
    /// may already be gone; the overflow guard is what keeps that true.
    fn mint_placement_id(&mut self) -> PlacementId;
}
```

`mint_placement_id` は `Grid::mint`（`grid.rs:252`）と同じイディオムを採る。単調前進と
`checked_add` によるオーバーフローガード、doc の `# Invariants` 節の書き方まで揃える。

`switch_screen` は **`modes.active_screen` を書き、その遷移が落とした placement を返す**。
1 回の呼び出しで遷移が完結する。

当初はモードを書かない案（呼び出し側が書き、本メソッドは teardown だけ）を採っていたが、
その根拠が誤っていた。「既存のテストフックと衝突する」は成立しない ——
`set_active_screen_for_test` は `#[cfg(test)]` で production ビルドに存在せず（`device.rs:143`）、
その doc 自身が「The production path is the `?1049` handler, which is not implemented yet;
this exists so the placement tests can reach the alternate screen」と書いている。
そこは production の遷移 API が入るべき穴である。

`switch_screen` という名前のメソッドが switch しない形は、`?1049` ハンドラが 2 回呼ぶ手順を
慣習でしか守れない。`VtModes::active_screen` は "the only record"（`device/modes.rs:17-22`）で、
`DeviceState` はモードとスクリーンの両方を持つ唯一の型なので、遷移はここで原子的に行う。

移設に伴って**述語そのものが消える**点は移設以上の効果がある。現行の
`evict_where(|p| p.screen != ScreenKind::Primary)`（`placement.rs:196`）は、
per-screen 所有では条件を持たない `alternate.take_placements()` に縮む。
`p.screen` を読む構造がこれで全て無くなる。

```rust
pub fn switch_screen(&mut self, to: ScreenKind) -> Vec<PlacementId> {
    self.modes.active_screen = to;
    match to {
        ScreenKind::Alternate => Vec::new(),
        ScreenKind::Primary => self.screens.alternate.take_placements(),
    }
}
```

「primary は alternate 表示中も隠れるだけ」を実現するコードは **1 行も要らない** ——
アクティブなスクリーンの表しか射影しないので、primary の placement はただそこに在り続ける。

なお **RIS は `switch_screen` を経由しない**。`ris.md` #3 の `DeviceState::reset` は
`VtModes::default()` でアクティブスクリーンを Primary に戻すが、placement の破棄は
両スクリーン sweep が担う。両方を通すと alternate の id が二重に名指される。
RIS の結線はスコープ外だが、この分担は本設計が前提にしている。

### `FrameTracker` と `Executor`

```rust
// frame.rs
pub fn emit(&mut self, device: &DeviceState) -> Option<Frame>   // 第 2 引数が消える

fn diff_placements(&self, device: &DeviceState) -> Option<Vec<AnchoredPlacement>> {
    let projected = device.active().project_placements();
    (projected != self.placements).then_some(projected)
}
```

```rust
// interpreter.rs
struct Executor<'a> {
    damaged: &'a mut bool,
    sync: &'a mut SyncBuffer,
    device: &'a mut DeviceState,
    // placements: &'a mut PlacementStore,   ← 消える
    tracker: &'a mut FrameTracker,
    signal_tx: &'a mut Sender<VtSignal>,
}
```

`OrzmaVt` からも `placements: PlacementStore` フィールドが消え、`Vt::frame` は
`self.tracker.emit(&self.device)` になる。

## eviction 経路の一本化

### 現行の設計

eviction は「操作ごと」ではなく「チャンクごとの掃除フェーズ」として設計されている。
`PlacementStore::project` の doc がこれを宣言している。

> Projection reads; it never evicts, repairs an anchor, or refreshes a cache. Those belong to
> `evict_lost_anchors`, which runs while damage can still be staged — a mutation here would land
> after the ledger was drained and reach no frame.

`Executor` は 1 チャンク 1 個（doc: "carries no state between chunks"）なので、`line_feed` が
100 回走っても sweep は 1 回。`line_feed` / `reverse_index` / `scroll_span` の返り値は
`Option<DamageSpan>` のままで、腐った anchor はチャンク末尾がまとめて回収する。

anchor が腐る契機は次のとおり。VT スクロールしたら腐る、ではなく、行が ring から出たら腐る。

| 操作 | anchor が腐るか | 位置 |
| - | - | - |
| `scroll_up_one(top=0)`、history に空きあり | 腐らない（history へ移り `Some(-1)` になる） | `grid.rs:172-177` |
| `scroll_up_one(top=0)`、history が cap | 最古の history 行が腐る | `grid.rs:178-186` の `pop_front` + 再 mint |
| `scroll_up_one(top>0)`（上にピン留めがある領域スクロール） | 出ていく行が即座に腐る | `grid.rs:162-170` |
| `scroll_down_one`（reverse index） | 最下行が腐る | `grid.rs:201-209` |
| `Grid::reset` | 全部腐る | `grid.rs:113-122` |
| `Vt::scroll`（ユーザースクロール、display offset） | 腐らない。行は動かない | `screen.rs:601-605` |

### 変更 — sweep を両スクリーンへ

現行の sweep はアクティブスクリーン限定で、`evict_lost_anchors` の doc がその制約と
将来の解除を既に宣言している。

> Only the active screen can be checked, which is sound while the inactive grid never scrolls.
> **Reflow breaks that and will have to sweep both.**

本設計はこれを前倒しする。`DeviceState::evict_lost_anchors` が両スクリーンを走査して連結する。

### 両スクリーン sweep のコスト

**タダではない。** `Grid::grid_line` は `rows.iter().rposition(|r| r.id == id)` のリニアスキャンで
（`grid.rs:219-225`）、production の `max_history` は 10,000（`bevy_orzma_tty/src/lib.rs:25`）。
現行はアクティブ判定が先に来る `&&` の短絡（`placement.rs:181`）により、非アクティブ側の
placement は**整数比較 1 回**で済んでいる。両スクリーン sweep はその短絡を失う。

| 状況 | 現行 | 本設計 |
| - | - | - |
| 非アクティブ側の表が空 | 比較 0 回 | 早期リターンで分岐 1 回 |
| 非アクティブ側に placement N 個、anchor が可視付近 | 比較 N 回 | 短いスキャン N 本 |
| 非アクティブ側に placement N 個、anchor が履歴の奥 | 比較 N 回 | **リングを奥まで N 本走査** |

最後の行が退行になる。具体例: primary に webview を mount し、出力で 9,000 行スクロールバックへ
押し込まれた状態で vim を開くと、vim が吐くチャンクごとに約 9,000 ステップの逆走査が 1 本走る。
検討時の「普段は何も見つからない」は正しいが、**生きた anchor を履歴の奥に見つけるのが
最も高い**ので、コスト評価としては誤りだった。

したがって:

- `ScreenPlacements::evict_lost_anchors` は空テーブルで早期リターンする（現行 `placement.rs:177-179` と同形）。
  これが「placement を持たないスクリーンはタダ」を担保する唯一の仕掛けになる
- placement を持つ非アクティブスクリーンのコストは**受け入れる**。`MAX_PLACEMENTS` は 12 で、
  sweep はチャンクごと 1 回。実測していないので「問題ない」とは言わず、「本設計では対処しない」とする

**採らなかった最適化: reset watermark。** `Grid` に reset 時点の `next_line_id` を保持し、
`grid_line` が `id < watermark` を O(1) で棄却する案がある。RIS は全 anchor が死んでいて
全スキャンが最悪長になるので効果が最も大きく、代償 (b) —— `Grid::reset` が `Grid::new` に
書き換えられると RIS が placement を落とさなくなる —— を invariant として検出できる副次効果もある。
採らない理由は本設計のスコープが所有権移動だからで、`Grid` に不変条件つきのフィールドを足すのは
別案件として扱う。なお scroll 由来の eviction では watermark を上げてはならない —— reverse index が
低い id の上に高い id を挿入するため（`grid.rs:201-209`）、単調な下界が成立しない。

**採らなかった最適化: 行 identity の invalidation フラグ。** `Grid` が「このチャンクで行の同一性を
捨てたか」を記録し、捨てていないスクリーンの sweep を丸ごと飛ばす案。identity が失われるのは
cap 到達スクロール・領域スクロール・reverse index・reset・将来の reflow だけで（`grid.rs:162-186`,
`:201-209`, `:113-122`）、通常のカーソル移動・印字・履歴が伸びるスクロールでは失われない。
watermark より効く場面が広く、非アクティブスクリーンはほぼ常にスキップされる。採らない理由は
watermark と同じで、加えて「今後 行を捨てる操作を足す人が必ずフラグを立てる」という不変条件が増える。
所有権移動が落ち着いてから、実測を伴って判断するのが妥当。

普段は非アクティブ側で何も見つからない（そのスクリーンの grid が動かないため）代わりに、
腐りうる 2 つの経路 —— RIS と、将来の reflow 付き resize —— を両方カバーする。

### 帰結 — `Screen::reset` は placement について何も返さない

`Grid::reset` は行を作り直すたびに `mint()` で採番し、counter を巻き戻さない
（`grid.rs:113-122`、`ris.md` 決定事項 1、テスト `a_reset_mints_ids_no_pre_reset_anchor_can_match` と
`a_reset_does_not_rewind_the_id_counter`）。したがって **reset 直後は全 anchor が解決不能**になり、
「全破棄」と「解決不能を全破棄」が一致する。

両スクリーン sweep と組み合わせると、RIS の placement 破棄は次の合成で成立する。

1. `DeviceState::reset` が両スクリーンの `Screen::reset` を呼び、両方の grid が id を振り直す
2. チャンク末尾の `DeviceState::evict_lost_anchors` が両スクリーンを走査し、全 placement を回収して id を返す
3. `Executor` がその id を `VtSignal::WebviewEvicted` に載せ、非空なら chunk liveness を立てる

よって:

- `Screen::reset` のシグネチャは `Option<DamageSpan>` のまま。既存テスト 12 箇所（`screen.rs:2468`-`:2644`）が無修正
- `ris.md` #4 の `PlacementStore::clear()` は**不要になる**
- `ris.md` #5 の RIS ハンドラは「`device.reset()` を呼んで damage を stage する」だけになり、
  placement 専用の liveness 操作と signal 送出が消える
- cap の解放タイミングが scroll 由来と揃う（どちらもチャンク末尾）
- liveness のルールが「sweep の結果が非空なら立てる」の 1 本になる

最後の点には具体例がある。overlay はセルを書かないので、**空の画面に webview だけが乗っている**
状態では `Grid::is_blank()` が true になり、`Screen::reset` は `None` を返す。それでも placement を
落として短くなったリストをフレームに載せる必要がある。sweep 起点の liveness はこれを含めて
全経路を覆う。

### この一本化の代償

**(a) 結合が暗黙になる。** `Screen::reset` のコードを読んでも placement が落ちることが見えない。
「`Grid::reset` が id を振り直す」＋「チャンク末尾に両スクリーン sweep が走る」の合成で初めて
成立する。緩和として `Screen::reset` の doc に明記する。

**(b) `Grid::reset` の挙動が RIS の正しさを支える。** 誰かが `Grid::reset` を「`Grid::new` で
作り直す」形に書き換えると id が 0 から振られ、古い anchor と衝突して **RIS が placement を
落とさなくなる**。`ris.md` 決定事項 1 のテストは `LineId` の衝突を見ているだけなので、
placement レイヤに専用の回帰テストを足す（下記 T-N1）。

**(c) cap がチャンク末尾まで解放されない。** 同一チャンク内で `ESC c` の直後に 12 個 mount すると、
まだ表が空いていないので拒否される。ただしこれは scroll 由来の解放と同じ挙動なので、
特例が減る方向の変化である。

## 消えるもの

| 対象 | 位置 |
| - | - |
| `PlacementStore` 型ごと | `placement.rs` |
| `Placement.screen: ScreenKind` | `placement.rs:233` |
| `ActiveScreen` 型ごと（4 メソッド + doc 込み 33 行） | `device.rs:149-181` |
| `DeviceState::active_screen()` | `device.rs:79-86` |
| kind 絞り込み 2 箇所 | `placement.rs:93`, `:181` |
| `switch_screen` の `evict_where(\|p\| p.screen != Primary)` の条件 | `placement.rs:196` |
| `Executor` の借用フィールド 1 個（6 → 5） | `interpreter.rs:101` |
| `FrameTracker::emit` の第 2 引数 | `frame.rs:139` |
| `Screen::viewport_row_of`（`ActiveScreen` 側は上の型ごと消える行に含まれる） | `screen.rs:614` |
| `ProjectedPlacement.viewport_row: i32`（`AnchoredPlacement.point: GridPoint` になる） | `placement.rs:38` |
| `OrzmaVt::placements` フィールド | `lib.rs:224` / `:245` / `:264` |
| 回帰テスト `a_sweep_leaves_the_other_screens_placement_alone` | `placement.rs:467` — 守る失敗モードが存在しなくなる |
| `ris.md` #4 の `clear()` API | 未実装のまま不要になる |

## テスト計画

### 移設

`placement.rs` の `mod tests` にある表操作のテストを `screen/placements.rs` へ移す。
`device()` ヘルパで `DeviceState` を組み立てて `store` と対にする現行の形は、
`Screen` 1 個で完結する形に単純化される。

| 現在の対象 | 移設先 |
| - | - |
| supersession、unmount のアドレス指定、`take_all` | `screen/placements.rs` の `mod tests` |
| mount が cursor に anchor する、射影が anchor を grid 行へ解決する、sweep が解決不能を落とす | `screen.rs` の 14 番目のテストモジュール |
| cap、id 単調、`switch_screen` の teardown | `device.rs` の `mod tests` |

### 削除

- `a_sweep_leaves_the_other_screens_placement_alone` —— 非アクティブ側の anchor を
  アクティブな grid で解決する経路が型として存在しなくなる
- `screen.rs:2260` の `mod viewport_row_of` 3 本。`Screen::viewport_row_of` が消えるため:
  - `an_anchor_above_the_viewport_reports_a_negative_row` と
    `an_anchor_trimmed_from_the_ring_stops_resolving` は `screen/grid.rs:421-449` の
    `grid_line` テストに吸収される
  - `scrolling_back_moves_an_anchors_reported_row_down` が固定していた「ユーザーがスクロールすると
    webview が追従する」契約は `orzma_vt` の外へ出る。移設先は `orzma_webview` の
    `project_webview_overlays` テスト（`mount.rs`）で、`display_offset` と `placements` を載せた
    `TerminalGrid` を組み、`TerminalOverlays` の rect の row を確かめる形になる

### 追加

| ID | 名前（案） | 何を固定するか |
| - | - | - |
| T-N1 | `a_screen_reset_leaves_every_placement_unresolvable` | 代償 (b) の回帰。両スクリーンに mount し、`set_active_screen_for_test` で切り替えながら各 `Screen::reset` を呼び、`evict_lost_anchors()` が全 id を返す。`DeviceState::reset`（`ris.md` #3）は未実装なので経由しない |
| T-N2 | `a_sweep_reaches_the_inactive_screen` | 両スクリーン sweep。非アクティブ側の腐った anchor が回収される |
| T-N3 | `a_mount_at_the_cap_is_rejected_across_both_screens` | cap が合算であること。片方 6 + もう片方 6 で 13 個目が拒否される |
| T-N4 | `a_remount_supersedes_across_screens` | アドレス空間が端末単位であること |
| T-N5 | `a_placement_the_projection_omits_is_also_swept` | 射影と sweep が同一の式で anchor を解決すること。anchor を殺した状態で `project_placements()` が省き、かつ `evict_lost_anchors()` が名指すことを 1 つのテストで対にする。リゾルバが 2 本に分かれる将来の変更をここで落とす |
| T-N6 | `a_broad_unmount_reaches_both_screens` | `unmount_placement` が短絡しないこと。同じ `view_id` を両スクリーンに mount し、`view_id` だけを指定した unmount で**両方**消えることを固定する。`.any(..)` や `a \|\| b` で書くと片方が残り、host は両方 despawn するので VT だけが cap スロットを抱える |

### `tdd-placement-reset.md` の 7 ケース

対象が `PlacementStore::reset()` 直呼びから `DeviceState` 経由に変わる。

| ケース | 統一案での扱い |
| - | - |
| TC-01 全破棄して id を名指す | reset → `evict_lost_anchors()` の 2 段で検証 |
| TC-02 viewport 外だが history に在る anchor | 依然として必要。reset 前は sweep が残し reset 後は落とす、を対比で固定。書き換え時に**禁止の理由が変わったことを明記する** —— 元の TC-02 は「reset が sweep の述語を流用する実装」を禁じるために書かれたが、本設計はまさにそれを採る。成立するのは `Grid::reset` が先に走って anchor を破壊するからで、その順序が根拠であることを残さないと将来の読み手が旧い禁止を再導出する |
| TC-03 非アクティブスクリーン | 本設計の要。T-N2 と統合してよい |
| TC-04 既に grid から消えた anchor | reset 前から `None` なので reset 有無に関わらず落ちる |
| TC-05 2 度目の reset は空 | そのまま |
| TC-06 cap 解放 | 「sweep の後に mount が受理される」に書き換え |
| TC-A1 id counter を巻き戻さない | 対象が `DeviceState::next_placement_id` に変わる |

## 実装上の注意

**本書のコードスケッチはそのまま転記できない。** 説明のために付けた注釈が
[`.claude/rules/rust.md`](../../../.claude/rules/rust.md) の comment taxonomy に反する。

| スケッチ中の記述 | 転記時 |
| - | - |
| `placements: ScreenPlacements,   // 8 番目` | 削る。plain narrative comment は禁止 |
| `// placements: &'a mut PlacementStore,   ← 消える` | 削る。commented-out code は禁止 |
| `size: PlacementSize,   // 実装済み` 相当の注釈 | 削る |

テストの doc も同様で、テスト計画の「何を固定するか」欄は説明文であってテスト doc の形ではない。
実際の `///` は「1 行目に主張、空行、`Case:` 段落のみ」に整える。

**`evict_where` は `Vec::extract_if` で書く。**

```rust
fn evict_where(&mut self, should_evict: impl FnMut(&mut Placement) -> bool) -> Vec<PlacementId> {
    self.placements.extract_if(.., should_evict).map(|p| p.id).collect()
}
```

現行の `retain` + アキュムレータ（`placement.rs:214-227`）と同じ操作を、反転した述語も
アキュムレータも無しで書ける。述語が `&mut Placement` を受けるので境界が
`FnMut(&mut Placement) -> bool` に広がる。1.95 / edition 2024 で動作確認済み。

## 移行手順

各段階で木がコンパイルでき、テストが通る形に分ける。段階 2〜4 の間は新旧の表が併存するが、
新しい側を読む本番コードが無いので二重管理にはならない（`device.rs` には既に
`#![expect(dead_code)]` がある）。

1. **placement を grid 空間で出す。** `ProjectedPlacement` を
   `AnchoredPlacement { id, point: GridPoint, size }` に置き換え、`PlacementStore::project` の
   戻り値と `Screen::viewport_row_of` / `ActiveScreen::viewport_row_of` の削除、
   `orzma_tty_renderer` の再エクスポートと `TerminalGrid.placements`、`orzma_webview` の
   `project_webview_overlays`（`mount.rs:603-618`）とそのテストリテラル、`screen.rs:2260` の
   `mod viewport_row_of` の処遇、`Frame.placements` と `ViewportLine` の doc 修正まで。
   所有権の移動とは独立しており、この段階だけで木が通る
2. **`screen/placements.rs` を新設。** `ScreenPlacements` と `Placement` を実装し、
   表操作のテストを `placement.rs` から移植する。この時点では誰も使わない
3. **`Screen` に 8 番目のフィールドと 14 番目の impl ブロックを追加。** mount / supersede /
   unmount / project / sweep / take_all とそのテスト
4. **`DeviceState` に採番・cap・アドレス空間・両スクリーン sweep を追加。** T-N1〜T-N4 を含むテスト
5. **読み手を切り替えて旧実装を削除。** 下表の全箇所を一度に切り替える。ここが唯一の
   破壊的ステップで、分割できない
6. **`Screen::reset` の doc に暗黙の結合を明記。** 代償 (a) の緩和

段階 5 で触る箇所は次のとおり。`PlacementStore` を引数・フィールドとして持つコードが
production とテストの両方にあり、**全て同時にしか切り替えられない**。

| 箇所 | 内容 |
| - | - |
| `frame.rs:139`, `:189`, `:192` | `emit` の第 2 引数を落とし、`diff_placements` を `device.active().project_placements()` へ |
| `frame.rs:240`, `:250`, `:256`, `:281`, `:391`, `:458` | テストの `Rig` が持つ `placements` フィールドと、`PlacementStore` を直接駆動する 3 テスト |
| `interpreter.rs:45`, `:101`, `:292`, `:300` | `Interpreter::parse` の `placements: &mut PlacementStore` 引数、`Executor` の借用フィールド、テストヘルパ |
| `lib.rs:224`, `:245`, `:264` | `OrzmaVt::placements` フィールドと `Vt::frame` |
| `lib.rs:354-378` | `a_screen_flip_replays_the_placement_list` —— `vt.placements.mount(vt.device.active_screen(), ..)` を呼んでいる |
| `placement.rs` | `PlacementStore` と `Placement` の削除、`MAX_PLACEMENTS` の `pub` 化 |
| `device.rs:79-86`, `:149-181` | `active_screen()` と `ActiveScreen` の削除 |
| `placement.rs:467` | `a_sweep_leaves_the_other_screens_placement_alone` の削除 |
| `device.rs:25-30` | `DeviceState` の doc が偽になる。「It owns no parser, **placement-extension**, damage, or emission state — those are the VT's own machinery and sit beside it in `OrzmaVt`」を、端末スコープの 3 つを持つ形へ書き換える |
| `frame.rs:15-17`, `interpreter.rs:20-25`, `lib.rs:7-16` | `PlacementStore` / `ActiveScreen` の import 除去 |

段階 1〜4 はいつでも中断でき、段階 5 に入ったら完走する。

## 追随が必要な文書

| 文書 | 変更 |
| - | - |
| `docs/todo/ris.md` | #4（`clear()`）を削除。#5 の RIS ハンドラを簡素化。決定事項 2 の記述を「sweep 経由で破棄」に |
| `docs/todo/tdd-placement-reset.md` | 上表のとおり 7 ケースの対象を変更、T-N1 を追加 |
| `docs/todo/placement-ownership.md` | 内容は本書が引き継ぐ。検討メモ側には「採用が決まった」旨と本書へのリンクだけを残す |
| `placement.rs` の `evict_lost_anchors` doc | "Reflow breaks that and will have to sweep both" が実現済みになる |
| `frame.rs:51-53` の `Frame.placements` doc | "Viewport-projected" を grid 空間へ。`Some(vec![])` の意味を「可視な placement が無い」から「anchor が生きている placement が無い」へ（カリングは元から消費側にある）。ユーザースクロールでは再送されなくなる旨も記す |
| `screen/viewport.rs:21-25` の `ViewportLine` doc | `u16` では表現できない負値と `-1` センチネルの記述を落とす |
| `device.rs:96-101` の reflow TODO | 「兄弟フィールドなので caller が配送する」必要が消え、各 `Screen` が自分の表を持つ形に書き換わる |

## 検討して採らなかった案

| 案 | 却下理由 |
| - | - |
| `ActiveScreen` を `&Screen` に置換 | `kind()` の呼び出し元が 3 箇所ある。placement を降ろさない限り消せない |
| `&Screen` と `ScreenKind` の 2 引数 | `ActiveScreen` の doc が unconstructible と言う取り違えを再導入する |
| `Screen` に `kind` フィールドを追加 | `VtModes::active_screen` の "the only record" と `Screens` の "pure storage" に衝突 |
| `Screen` が `Vec<Placement>` を直接持つ | 表の不変条件が VT 操作と同じ impl 群に混ざる。7 フィールド全てが `screen/` に型を持つ慣習からも外れる |
| `PlacementStore` を `placement.rs` に残して `Screen` のフィールドにする | 同上。`Screen` のフィールドで型が `screen/` 外にある唯一の例外になる |
| `Screen::reset` が `(Option<DamageSpan>, Vec<PlacementId>)` を返す | sweep 一本化で不要になった。13 ブロック中このメソッドだけがタプルを返す形も避けられる |
| `Screen::reset` が evicted を内部バッファし `take_evicted()` で drain | 「変更は返り値で表す」という現在の作りから外れる。sweep 一本化で問題ごと消えた |
| RIS が sweep を待たず即座に両スクリーンを clear | 特例が 3 つ（専用 API・専用 liveness・専用戻り値）増える。cap 解放のタイミングも scroll 由来と食い違う |
| `instance_id` を VT が採番して `PlacementId` を廃止 | supersession が死に、再 mount のたびに host の entity が despawn → ページリロード。加えて `mount` が fire-and-forget でなくなる。`PlacementId` はクライアントプロトコルに露出していない（`orzma_webview_protocol.md` に 0 件）ので、内部の重複 1 個のためにプロトコルの性質を変える交換になる |
| `Viewport::row_of(GridLine) -> i32` を足し、`Screen` の private リゾルバ `anchor_row_of(grid, viewport, id)` で grid 引きとオフセット適用を合成する | 当初案。VT が placement を viewport 空間で出す前提でのみ必要だった。grid 空間で出す形（`AnchoredPlacement`）にすると生死判定が `Grid::grid_line` 1 つに縮み、`Viewport::row_of` も `anchor_row_of` も要らなくなる。射影と sweep が同一の式を共有するという当初の目的はそのまま満たされ、`Screen` の private helper が 1 つ減る |
| `DeviceState` に `both() -> [&Screen; 2]` / `both_mut() -> [&mut Screen; 2]` を置き、ペアに跨る操作をそこへ通す | フィールドが 2 つしか無い型に private helper を 2 つ足し、毎回一時配列を経由することになる。`self.screens.primary` と `.alternate` を並べて書くほうが短く、`unmount_placement` に至っては「短絡してはならない」不変条件が 2 本の束縛として構造に現れる。primary-first の順序が観測されるのは `evict_lost_anchors` の戻り値だけなので、順序の契約はそのメソッドの `# Invariants` に置いた |
| `GridLine` に非カリングの射影メソッド（`to_viewport_row(offset) -> i32` 等）を足す | `GridLine` の変換口が 2 つになり、カリングする `to_viewport` との使い分けと `ViewportLine` との区別が曖昧になる。符号付きの行が要るのは `orzma_webview` の 1 箇所だけで、逆方向は既に `orzma_tty_renderer/src/grid.rs:44`, `:86` でベタ書きされている |
| `Vec<Placement>` を `ArrayVec<Placement, 12>` に | cap は**両スクリーン合算**なので per-screen の型パラメータでは表現できず、24 を許してしまう。`Vec::new()` は最初の push まで確保しないので空表のコストも既にゼロ。依存が増えるだけで不変条件を得ない |
| クロージャの代わりに `trait AnchorResolver` | 単相化は同じで、`ScreenPlacements` にトレイト境界を通す分だけ増える。crate は既に `evict_where` で述語をクロージャで渡している |
| `HashMap<LineId, usize>` の側インデックスや `slotmap` / `generational-arena` | 索引は `VecDeque::insert` / `remove` のたび、つまりスクロールのたびに再構築が要り、支配的なケースでスキャンより遅い。arena は「同一性は行に、アドレスはスクロールで動く」というリングの性質と逆を向く |
| `FrameTracker` に射影のスクラッチバッファを戻す | `14c5556` で収支を数えた上で削除したばかり。差分ありの経路では `clone` が消えて同数、placement 0 個なら `collect` も確保しない |
| `placement.rs` を完全に解体する（`AnchoredPlacement` を `frame.rs` へ、`MAX_PLACEMENTS` を `device.rs` へ、残りを `screen/placements.rs` へ） | 方向としては正しい。4 項目 3 所有者の状態は本設計が 1 階層上で消している匂いと同じ。ただし段階 5 の差分をさらに広げるので **follow-up として分離する** |

## 本設計が前提にしている、まだ結線されていないもの

いずれもスコープ外だが、本設計の正しさが依存している。

| 前提 | 現状 |
| - | - |
| `Executor` がチャンク末尾に `DeviceState::evict_lost_anchors` を呼ぶ | 呼び出し元が無い。`Vt::interpret` が `todo!()`（`lib.rs:260`） |
| `Executor` が evicted id を `VtSignal::WebviewEvicted` に載せる | signal の outbox 自体が TODO（`interpreter.rs:91`） |
| sweep の結果が非空なら chunk liveness を立てる | 同上（`interpreter.rs:93-97`） |

VT より下（`Grid` のトリム、`grid_line` の `None`、表の sweep）と、
VT より上（`signals.rs:150` の pump、`mount.rs:463` の `on_webview_evicted`）は実装済みで
テストもある。欠けているのは `Executor` の 1 段だけで、本設計はその段が呼ぶ API の形を
確定させる。
