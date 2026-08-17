# OrzmaVt 内部構造設計(フィールド構成)

調査日: 2026-08-17

自前実装する `OrzmaVt`(`crates/orzma_vt/src/lib.rs` の `Vt` トレイト実装)の内部フィールド構成を、処理と機能の分離を軸に設計した。Claude 案を Codex にレビューさせた合議結果であり、その後確定した placement 契約(`Vt::interpret` の「Webview placements」節、`ProjectedPlacement` 一覧方式)に整合させてある。

## 1. 前提

- 満たす契約は `Vt` トレイト(`crates/orzma_vt/src/lib.rs`)のみ: `interpret → VtUpdate`、`frame`、`resize`(reflow)、`scroll`、`grid_size` / `display_offset` / `modes`。読み出し面はフレーム粒度(`Row`/`Run`)のみで、セル粒度の read seam(旧 `cell_at`)は設けない — hover 等のセル単位機能は emit 済みの `Row`/`Run` に対してホスト側で解決し、storage セルはクレート外に出さない(セル実体化型 `GridCell` はレンダラ語彙として `orzma_tty_renderer::schema` が所有)。
- Selection / vi モードは後日 capability トレイトとして追加する。レイアウトはその余地を残す。
- webview は **VT が `PlacementId` を採番して placement テーブルを所有し、毎 emit でビューポート射影済みの一覧(`FrameSnapshot::placements` / `FrameDelta::placements`)を配る**契約。`history_base` の絶対行投影と seq 回り込み比較は廃止済みで、フレーム外部に露出する履歴カウンタは存在しない。
- schema の既存語彙(`Damage` のマージ代数、`DamageVerdict::classify`、`Row`/`Run` + style ビット、`Palette`、`VtModes`、`VtSignal`、`PlacementId` / `ProjectedPlacement`)と `HyperlinkInterner` を再利用する。

## 2. トップレベル構成

```rust
pub struct OrzmaVt {
    /// vtparse パーサ + CSI ?2026 同期更新バッファ。
    interpreter: Interpreter,
    /// 端末状態の本体: Screens(primary/alternate)+ ModeState +
    /// TabStops + ColorTable + タイトルスタック。
    terminal: TerminalState,
    /// webview 配置テーブル: 採番・アンカー追従・eviction・射影。
    placements: PlacementStore,
    /// 次フレームまでの staged damage(schema `Damage` のマージ)。
    damage: DamageLedger,
    /// フレーム発行: wrapping seq・first_emit・HyperlinkInterner・
    /// Row/Run 構築・placement 射影の呼び出し。
    emitter: FrameEmitter,
}

struct TerminalState {
    screens: Screens,   // { primary: Screen, alternate: Screen, active }
    modes: ModeState,   // DECSET 群 + 内部モード(insert/origin/autowrap …)
    tabs: TabStops,
    colors: ColorTable, // 基本パレット + OSC 4/10/11/12 動的上書き
    title: TitleState,  // 現在タイトル + タイトルスタック(CSI 22/23 t)
}

struct Screen {
    grid: Grid,          // セル格納 + スクロールバックリング(storage 専任)
    viewport: Viewport,  // display_offset(alternate は常に 0 にクランプ)
    write: WriteState,   // カーソル位置・pending wrap・ペン(SGR + OSC 8 リンク)・チャーセット
    saved: SavedCursorSlots, // DECSC / ANSI 保存スロット(スクリーン毎)
    margins: Margins,    // DECSTBM / DECSLRM スクロール領域
}
```

`interpret` 毎のシグナル・応答は永続フィールドにせず、呼び出しローカルの `Products`(outbox)に集める(§4.6)。

## 3. 各コンポーネントの責務

| コンポーネント | 責務 | 責務外 |
|---|---|---|
| `Interpreter` | バイト列→アクションのデコード(vtparse)、`?2026` 同期更新のバッファリング | 状態変更(Executor 経由) |
| `Executor`(一時ビュー) | パーサコールバックの実装。分割借用した各コンポーネントへアクションを適用 | 永続状態の保持 |
| `Grid` | セル格納・スクロールバックリング・reflow の**ストレージ操作**。操作は `HistoryEvent` と damage 効果を返す | カーソル・ビューポート・モード |
| `Viewport` | `display_offset` の保持とクランプ | セル内容 |
| `WriteState` | 書き込みカーソル・pending wrap・SGR/OSC 8 ペン・G0–G3 チャーセットとシフト状態 | 格納 |
| `Screen` | grid + カーソルを**原子的に**更新する操作単位(行送り・スクロール領域内スクロール・reflow) | 発行・damage 集約 |
| `ModeState` | DECSET フラグ(`VtModes` の供給源)+ 内部専用モード | — |
| `ColorTable` | パレットと動的カラー上書き、`Palette` の供給 | — |
| `PlacementStore` | `PlacementId` 採番、`(view_id, instance)` → 配置、行アンカー追従、占有スパン、eviction 判定、ビューポート射影 | GUI ポリシー(registry 照合等はホスト側) |
| `DamageLedger` | 全ソース(interpret / scroll / resize / placement 変化 / 将来の selection)の staged damage を一元マージ | — |
| `FrameEmitter` | seq(wrapping、emit 時のみ前進)・初回/alt 切替の Snapshot 保証・Row/Run 構築・`HyperlinkInterner`・placement 射影の組み込み | 状態変更 |

## 4. 主要な設計判断と根拠

### 4.1 カーソルは Screen 毎(sibling でも Grid 内でもない)

primary / alternate はカーソル・pending wrap・ペン・保存スロットを独立に持つため、単一の sibling カーソルは成立しない。一方 alacritty のように Grid へ埋めると storage と書き込み状態が癒着する。**`Grid` は storage 専任、reflow・スクロール・行挿入は「grid とカーソルを原子的に更新する Screen の操作」**とする。

### 4.2 `display_offset` は `Viewport` として分離

グリッド座標は「ユーザースクロールに不変」(`schema/grid.rs` の `GridLine` 契約)であり、offset は表示・ナビゲーション状態であって storage ではない。alternate 側 `Viewport` は常に 0 にクランプ。

### 4.3 単一パーサ + Executor パターン

旧エンジンの 3 パーサ fan-out(lead APC パーサ)は「alacritty の processor が不透明」なことへの補償だった。自前実装では **単一の `vtparse` + `Executor`(パーサ以外のフィールドを分割借用する一時ビュー構造体)** で APC は同期的に届く。

```rust
let Self { interpreter: Interpreter { parser, sync }, terminal, placements, damage, emitter } = self;
let mut out = Products::default();
let mut exec = Executor { sync, terminal, placements, damage, next_seq: emitter.next_seq(), out: &mut out };
parser.parse(chunk, &mut exec);
```

`?2026` バッファは所有データとして持ち、APC mount はバッファ済み作業を適用してから placement を確定する(「APC バイト位置のカーソルでサンプリング」契約の実現手段)。

### 4.4 DamageLedger は中央集約

damage は「アクティブビューポート + カーソル + 将来の selection/vi オーバーレイ」の概念で、特定 grid に属さない。Grid/Screen の操作は damage 効果を**戻り値で返し**、Executor が ledger に stage する。スクリーン切替は Full を stage(「alt 切替は必ず Snapshot」不変条件の実装点)。schema `Damage` の `BitOrAssign` マージがそのまま使える。

### 4.5 履歴簿記はイベント駆動(frozen ベースライン方式の廃止)

旧エンジンの `frozen_history` / `frozen_valid` は alacritty を外から観測するためのワークアラウンドであり、採用しない。**Grid 操作が `HistoryEvent::{Pushed, Evicted, Cleared, Reflowed}` を正確に返し**、`PlacementStore` が受けて処理する:

- `Evicted` / `Cleared` — アンカー行を失った placement を除去し `VtSignal::WebviewEvicted` を発行(旧・合成 unmount-all の後継。同一チャンク内の後続 mount より前に並ぶ)
- `Reflowed` — アンカーの再配置(eviction ではない)
- placement の増減・射影ジオメトリ変化は damage を stage(「変化は必ず emit が追従する」契約)

実装は eviction を独立イベントではなく push の変種 `HistoryEvent::{Pushed, PushedWithEviction}` として開始した(`crates/orzma_vt/src/screen/grid.rs`)。独立した `Evicted` / `Cleared` / `Reflowed` は、それらを発生させる操作(履歴クリア・reflow)の実装時に追加する。

なお現行契約では `history_base` を外部に配らないため、専用の HistoryLedger フィールドは不要になり、簿記は PlacementStore に収まる(合議時点からの簡素化)。

### 4.6 シグナル・応答は呼び出しローカルの `Products`

vtparse のコールバックは戻り値を持てないため、`Executor` が借用する per-interpret の `Products { signals, replies }` に収集し、`VtUpdate` へ変換して返す。永続 drain 状態を持たないことで「空チャンク → `VtUpdate::default()`」の契約が構造的に保証される。

### 4.7 webview はテキストセルの変種にしない

placement は Grid の行に振る**安定 `LineId`** にアンカーし、`PlacementStore` が side table として所有する。行毎の占有スパンを保持して erase / scroll / eviction / reflow に追従し、emit 時にのみ `ProjectedPlacement`(viewport 射影)へ変換する。セル変種にすると文字書き込み・reflow がセルを壊す経路が無数に生じる。resize(reflow)で射影が動くケースは「一覧を毎 emit 配る + 変化が emit を強制する」契約が吸収する。

## 5. 実装時に必要な状態(草案から漏れやすいもの)

スクロール領域(DECSTBM / DECSLRM)/ insert・origin・newline・autowrap 等の内部モード / タブストップ / G0–G3 チャーセット + シフト状態 / スクリーン毎の DEC・ANSI 保存スロット / protected・selective erase / wrap マーカーとワイド文字(スペーサ)不変条件 / UTF-8・grapheme の合成(zerowidth)/ OSC 8 の「現在リンク」ペンとライフサイクル / タイトルスタック(CSI 22/23 t)/ DECSCUSR カーソル形状。

発行規則として: **モード・パレットの変化は Snapshot を強制する**(delta は `modes` / `palette` を運ばないため)。Kitty graphics / keyboard・sixel は DCS/APC ハンドラの拡張点のみ確保し、ラスタ配置をテキストセルに入れない方針を placement と共有する。

## 6. モジュール配置(mod.rs 禁止規約準拠)

```
crates/orzma_vt/src/lib.rs               … pub struct OrzmaVt + impl Vt(現状はスタブ)
crates/orzma_vt/src/interpreter.rs       … Interpreter(vtparse + ?2026)〔未着手〕
crates/orzma_vt/src/executor.rs          … Executor(コールバック実装)+ Products〔未着手〕
crates/orzma_vt/src/terminal.rs          … TerminalState / ModeState / TabStops / ColorTable / TitleState〔未着手〕
crates/orzma_vt/src/screen.rs            … Screen / Viewport / WriteState / SavedCursorSlots / Margins / Effects〔実装済み〕
crates/orzma_vt/src/screen/grid.rs       … Grid / Row / HistoryEvent〔実装済み。LineId は placement 着手時に追加〕
crates/orzma_vt/src/screen/cell.rs       … Cell / Pen(schema GridCell とは別の内部表現)〔実装済み〕
crates/orzma_vt/src/placement.rs         … PlacementStore / 占有スパン〔未着手〕
crates/orzma_vt/src/damage.rs            … DamageLedger〔未着手〕
crates/orzma_vt/src/frame.rs             … FrameEmitter(Row/Run 構築・射影・seq)〔未着手〕
```

## 7. 実装順の示唆

1. `Grid` + `Screen` + `WriteState`(印字・行送り・erase の最小セット)と `DamageLedger` — `interpret` → `frame` の縦一本を最短で通す
2. `FrameEmitter`(Snapshot のみ → Delta 追加)
3. `ModeState` / `ColorTable` / タブ / チャーセット / スクロール領域
4. `Interpreter` の `?2026` と APC、`PlacementStore`(採番 → 射影 → eviction)
5. reflow(`Reflowed` イベント込み)
6. capability トレイト(selection / vi)は全て安定後

## 8. 参考

- 契約: `crates/orzma_vt/src/lib.rs`(`Vt` / `VtUpdate`、「Webview placements」節)、`crates/orzma_vt/src/schema/webview.rs`(`PlacementId` / `ProjectedPlacement`)、`schema/signal.rs`(`ApcWebview` / `WebviewEvicted`)
- 旧エンジン参照: `crates/orzma_tty_engine/src/handle.rs`(3 パーサ fan-out・frozen ベースライン — いずれも本設計では不採用)、`vt/frame_builder.rs`(Row/Run 構築の移植元)
- 経緯: Claude 草案 → Codex レビュー(TerminalState → Screen → Grid 階層、イベント駆動履歴、LineId + side table の提案)→ placement 契約確定後の整合(HistoryLedger の PlacementStore への縮退)
