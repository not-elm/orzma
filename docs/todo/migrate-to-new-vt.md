# 新 VT スタックへの移行 — 残作業

`orzma_tty_engine`（PTY + alacritty_terminal + Bevy 統合の一体クレート）を、
`orzma_vt`（VT エミュレーション）+ `orzma_tty`（PTY を持つコア）+
`bevy_orzma_tty`（Bevy 統合）の三層に置き換える。

- `orzma_tty_engine` を削除し、`bevy_orzma_tty` に差し替え
- `orzma_webview` を `bevy_orzma_webview` にリネーム

## 方針

**vi モードと選択は、切り替えを済ませてから実装する。** これらは切り替えの
ブロッカーとして扱わない。詳細は「切り替え後に実装」の節。

したがって切り替えの完了条件は「vi モードと選択を除いて、旧 engine と同じことが
できる」であり、切り替え直後は vi モードと選択が一時的に動かない状態を受け入れる。

## 現状: 新スタックは動く。残るのは接続作業

`src/main.rs` は今も `orzma_tty_engine::TerminalHandlePlugin` を配線している。

以前のブロッカーは「新スタックが動かない」ことだったが、それは解消した。`orzma_vt` に
`todo!()` は1つも残っておらず、workspace の `#[ignore]` もゼロ、画面には色が出る。
**いま残っているのは、新スタックを `src/` に繋ぎ直す作業である。** 代替画面は配線済みで、
vim / less で実用検証できる。

各クレートのビルド状態（`cargo check -p <crate> --all-targets`）:

| クレート | 状態 |
| --- | --- |
| `orzma_vt` | OK |
| `orzma_tty` | OK |
| `bevy_orzma_tty` | OK |
| `orzma_tty_engine` | **FAIL**（移行とは無関係の既存破損。`orzma_tty_renderer::prelude::ViewportPoint` の未解決 import と `Entity` の未 import） |

`orzma_tty_engine` が壊れているため **`cargo build` は今この瞬間も通らない**。移行は
「余裕があればやる改善」ではなく、アプリが再びビルドできるようになる唯一の道筋である。

---

# 切り替えのブロッカー

## 1. 端末を1つも構築できない → 解消済み

`DeviceState::resize` と `scroll` はどちらも実装済み。`OrzmaTty::spawn` /
`detached` が構築時に呼ぶ `vt.resize(...)` はもう panic せず、`OrzmaTtyHandle`
を作れるようになった。**workspace に `#[ignore]` はもう1件も無い。**

`resize` はリフローせず切り詰める方式を採った。関連する仕様の洗い出しは
[`resize-spec.md`](resize-spec.md) にまとめてある。要点として、リフローは VT510
にも xterm ctlseqs にも規定が無く（VT510 の答えは「切り詰める」）、placement
アンカーの行再割り当ては、リフローを入れると判断したときに初めて必要になる。

## 2. VT の実装 → 実用域に入った

`csi_dispatch` は8アームから **22アーム**になり、配線済みの終端バイトは
`A B C D E F H I J K W Z c f g h l m r t`。

| 追加された制御機能 | |
| --- | --- |
| SGR（`m`） | 色と装飾。**画面がモノクロでなくなった** |
| ED / EL（`J` / `K`） | 画面・行の消去 |
| CUU / CUD / CUF / CUB（`A`–`D`） | 相対カーソル移動。マージンが障壁 |
| CNL / CPL（`E` / `F`） | 行頭付き縦移動 |
| CHT / CBT（`I` / `Z`） | タブストップ単位の移動 |
| TBC / CTC / DECST8C（`g` / `W`） | タブストップの編集 |

`set_private_modes` は2モードから **14モード**に。DECCKM（`?1`）、代替画面（`?47` / `?1047` /
`?1048` / `?1049`）、マウストラッキング（`?1000` / `?1002` / `?1003`）、フォーカス報告（`?1004`）、
SGR マウス（`?1006`）、Alternate Scroll（`?1007`）、Bracketed Paste（`?2004`）。
**vim や less が動くようになった。**

**まだ未配線で、切り替えに効くもの:**

- **`?7`（DECAWM）/ `?25`（DECTCEM）** — `VtModes` にフィールドが無く、`Screen::print` と
  `Screen::cursor` の振る舞い変更を伴う
- **`?1005`（UTF-8 マウス）** — 意図的に答えていない。`MouseReport::encode` が `Utf8` を
  X10 にフォールバックさせるため、配線すると96桁を超えるウィンドウで座標が壊れる
- **絶対指定の CHA / VPA / HPA（`G` / `d` / `` ` ``）** — `move_cursor_to` の再利用で済む
- **文字編集の ICH / DCH / IL / DL / ECH（`@` / `P` / `L` / `M` / `X`）** — `Screen` 側にも
  実装が無い
- **DSR（`n`）** — カーソル位置報告。返信経路は DA1/DA2 で確立済み

`osc_dispatch` はタイトル（OSC 0 / 2）のみ実装済み。パレット（OSC 4 / 10 / 11 / 12）、
作業ディレクトリ（OSC 7）、ハイパーリンク（OSC 8）、クリップボード（OSC 52）は未実装で、
`VtSignal` の `Clipboard` / `CurrentDir` は発火元を持たない。`apc_dispatch` も
空実装（`fn apc_dispatch(&mut self, _data: Vec<u8>) {}`）なので `WebviewApc` も同様。

なお SGR は `Pen` が表現できる属性だけを実装し、点滅（`5` / `6` / `25`）・上線
（`53` / `55`）・下線の色（`58` / `59`）・下線の種類（`4:1`–`4:5`）は意図的に落として
いる。設計の経緯は `docs/memo/decset.md` と実装の doc に記録がある。

## 3. 未移植のモジュール

| 旧 engine | 新スタック | 状況 |
| --- | --- | --- |
| `buttons.rs` | なし | クリックがローカル選択になるかマウスプロトコルのバイト列になるかを決める routing。低レベルの SGR/X10 エンコーダだけが `orzma_tty/src/input/mouse.rs` に移植済みで、**判断ロジックは未移植**。選択側の分岐は切り替え後に回せるが、マウス報告側は切り替えに必要 |
| `wheel.rs` | `orzma_tty/src/input/wheel.rs` | 201行あるが**非コメント行が0行**。`input.rs` から公開もされていない完全な死蔵コード。`alacritty_terminal::TermMode` を `orzma_vt::VtModes` に読み替えて復活させる |
| `palette.rs` | なし | `orzma_vt` の `Rgb` を `bevy::Color` に変換する箇所がレンダラ側に存在しない |
| `title.rs` | なし | `sanitize_title` と永続タイトルコンポーネント。**`orzma_vt` 側に強化版のサニタイザが入ったが、実際のウィンドウタイトルを駆動しているのは今も旧 `title.rs` の弱い方**（U+061C 欠落、strip-before-trim なし） |
| `input_codec.rs` | `orzma_tty/src/input/keyboard.rs` | **移植済み。** 優先順位も同一で、キーパッド対応が追加されている |

## 4. `src/` の ECS 形状の書き換え

これは機能不足ではなく設計判断。旧 engine は `TerminalHandle` / `PtyHandle` /
`Coalescer` を**独立したコンポーネント**として公開しており、`src/` はそれぞれを
個別にクエリしている。

- `src/action/terminal.rs:50-66` — `Option<&mut PtyHandle>` と `Option<&mut Coalescer>`
- `src/action/vi/mode.rs:55` — `Query<(&mut TerminalHandle, &mut Coalescer)>`
- `src/ui/vi_mode_indicator.rs:285` — `entity.take::<Coalescer>()`

新スタックはこれらを不透明な `OrzmaTtyHandle` 1つに閉じ込めているため、
`action/terminal.rs`、`action/clipboard/paste.rs`、`action/vi/*`、`session/layout.rs`、
`ui/vi_mode_indicator.rs` の呼び出し側は全面的な書き換えが要る。

型名の対応も変わる。alacritty の re-export だった `Point` / `Column` / `Line` /
`Side` / `SelectionType` / `TermMode` / `ViMotion` は、それぞれ
`orzma_vt::prelude` の `GridPoint` / `GridColumn` / `GridLine` / `CellSide` /
`SelectionKind` / `VtModes` に対応する（`ViMotion` は `orzma_vt` にまだ無く、
`bevy_orzma_tty/src/requests/vi_motion.rs` が「`orzma_vt` が型を持つまで」の
断りつきで重複定義している）。

## 5. 受け手だけ先行している箇所

`bevy_orzma_tty/src/signals.rs` は `VtSignal` の8バリアント全てを Bevy イベントに
変換しているが、**そのイベントを観測するコードがリポジトリ内に1つも無い**。
`src/window_title.rs` は今も `orzma_tty_engine::TerminalTitle` を読んでいる。

タイトルを実際に画面へ出すには、`bevy_orzma_tty` 側にタイトルを保持する
コンポーネントと observer が要る（旧 `orzma_tty_engine/src/title.rs` の
`TerminalTitle` に相当するもの）。

---

# 切り替え後に実装

## vi モードと選択

`bevy_orzma_tty` の8プラグインのうち、選択と vi モードは**イベントを受け取って
何もしない**。

```rust
fn apply_vi_mode(_e: On<RequestTtyViMode>) {}                              // vi_mode.rs:25
fn apply_vi_motion(_e: On<RequestTtyViMotion>) {}                          // vi_motion.rs:77
fn start_selection(_e: On<RequestTtySelectionStart>) {}                    // selection.rs:84
fn start_selection_at_vi_cursor(_e: On<RequestTtySelectionStartAtViCursor>) {}
fn update_selection(_e: On<RequestTtySelectionUpdate>) {}
fn clear_selection(_e: On<RequestTtySelectionClear>) {}
```

`Vt` トレイトに選択と vi モードの capability が無いため、`orzma_vt` 側の設計から
必要になる。あわせて `SelectionKind`（`crates/orzma_vt/src/selection.rs:54`）は
`Simple` / `Lines` の2種しかなく、alacritty の `SelectionType` が持つ `Block` と
`Semantic` は `// TODO:` のまま。

### 切り替え時に決めておくこと: 読み取り側の受け皿

書き込み側（`enter_vi_mode`、`selection_start`、`selection_update_to`、
`selection_clear` など）は `RequestTty*` イベントを撃つだけなので、observer が
空のままでも「何も起きない」で済む。**読み取り側にはその逃げ道が無い。**

`src/` が `TerminalHandle` に対して行う選択/vi 呼び出し27箇所のうち、3種類は
値を返す読み取りになっている。

| 呼び出し | 箇所数 | 用途 |
| --- | --- | --- |
| `selection_to_string()` | 3 | コピー対象の文字列 |
| `selection_type()` | 4 | 現在の選択種別 |
| `vi_indicator_snapshot()` | 2 | vi インジケータの表示状態 |

`bevy_orzma_tty` が公開しているのは `OrzmaTtyHandle::new` と `::detached` の
**2つだけ**で、内部の `Vt` を覗く口が無い（`OrzmaTty::vt()` は存在するが
`OrzmaTtyHandle` からは辿れない）。`RequestTty*` はホスト → tty の一方向コマンドの
モデルなので、これら3つの代替にはならない。

切り替えの際にどちらかを選ぶ:

- **スタブで通す** — 空文字列 / `None` を返す暫定 API を `bevy_orzma_tty` に置く。
  コピーは何もコピーせず、vi インジケータは非表示になる
- **読み取り口を先に用意する** — `OrzmaTtyHandle` に `Vt` への読み取りアクセスを
  足し、実装は後から埋める

## 未実装のまま残る機能の明示

切り替え直後に一時的に失われるものを、切り替えの PR に書き出しておく:

- 選択（マウスドラッグ、ダブル/トリプルクリック、コピー）
- vi モード（入力、モーション、インジケータ表示）
- `buttons.rs` の選択側 routing

---

# 着手順

**切り替えまで:**

1. ~~**`DeviceState::resize` / `scroll`**~~ — 実装済み
2. ~~**`csi_dispatch` に SGR / ED / EL / 相対カーソル移動**~~ — 実装済み。8→22アーム
3. ~~**`set_private_modes` の拡張**~~ — 2→14モード
4. ~~**`?1049`（代替画面）**~~ — 実装済み（47 / 1047 / 1048 / 1049）。placement は発生源で
   `WebviewEvicted` にした。設計は `docs/memo/decset.md` の代替画面の節
5. **`osc_dispatch` の残り** — パレット、cwd、ハイパーリンク、クリップボード。
   `apc_dispatch` も空実装なので webview が出ない
6. **`wheel.rs` の復活と `buttons.rs` のマウス報告側の移植** — `mouse_tracking` は
   書き手ができたが読み手がまだいない。マウス転送は現在も無条件
7. **タイトルコンポーネントと observer**（ブロッカー5節）と `sanitize_title` の統合
8. **`src/` の ECS 形状の書き換え** — 分量は最大だが、設計判断は「読み取り側の受け皿」の
   1つに絞られている。22ファイル36箇所
9. **`orzma_tty_engine` の削除、`orzma_webview` → `bevy_orzma_webview` のリネーム**

**あると望ましいが切り替えの必須ではないもの:**

- 絶対指定の CHA / VPA / HPA — `move_cursor_to` の再利用で安い
- 文字編集の ICH / DCH / IL / DL / ECH — `Screen` 側から実装が要る
- DSR（カーソル位置報告）— 返信経路は確立済み
- DECAWM / DECTCEM — `VtModes` へのフィールド追加を伴う

**切り替え後:**

9. **`Vt` トレイトへの選択 / vi モードの capability 追加**
10. **`SelectionKind` に `Block` / `Semantic` を追加**、`ViMotion` を `orzma_vt` へ移す
11. **6つの空 observer の実装**と、読み取り側3種の実装
12. **`buttons.rs` の選択側 routing の移植**
