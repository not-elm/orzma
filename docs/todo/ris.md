# RIS

RIS（`ESC c`、`1B 63`）は端末を初期状態へ戻す制御関数であり、terminfo の `rs1=\Ec` として `reset(1)` や `tput reset` が送出する。実装の優先度と位置づけは [esc-dispatch.md](esc-dispatch.md) に記したとおりで、ここでは実装に必要な API を洗い出す。

一次情報は [vt510](../references/vt510.pdf) p.331 の RIS Actions を用いる。ESC と C1 の対応そのものは [esc.md](../memo/esc.md) にまとめてある。

## 追加が必要な API

| # | 追加先 | シグネチャ | 内容 |
| - | - | - | - |
| 1 | `screen/grid.rs` | `pub(crate) fn reset(&mut self)` | 履歴を捨て、可視行を `Cell::default()` で作り直す。`size` と `max_history` は据え置き、`next_line_id` も**引き継ぐ**（後述の決定事項）。現在の pub API は `history_len()` までで、履歴を捨てる手段が無い。 |
| 2 | `screen.rs` | `pub fn reset(&mut self)` | `Screen` の全7フィールドを初期状態へ戻す。内訳は次節の表のとおりで、`grid` だけが新規 API（#1）を要し、残りは既存の口で足りる。 |
| 3 | `device.rs` | `pub(crate) fn reset(&mut self) -> DamageSpan` | 両スクリーンへ #2 を適用し、`modes` を `VtModes::default()` へ戻す。返り値は `DamageSpan::Full` 固定とし、`Option` にはしない（RIS は必ず全面を書き換えるため）。`active()` / `active_mut()` しか無いので非アクティブ側へ届く口が現状は無いが、`screens` フィールドを持つ `DeviceState` 自身の実装であれば新しい到達手段は要らない。 |
| 4 | `placement.rs` | `pub(crate) fn clear(&mut self) -> Vec<PlacementId>` | 全 placement を破棄し、その id を返す。既存の private な `evict_where` をそのまま使える。`unmount` / `evict_lost_anchors` / `switch_screen` はいずれも条件付きで、全消しの口が無い。 |
| 5 | `interpreter.rs` | `Executor::esc_dispatch` の `(b'c', [])` アーム＋private な handler | #3 と #4 を呼び、`tracker` へ `DamageSpan::Full` を積み、chunk liveness を立て、#4 の返り値が空でなければ `VtSignal::WebviewEvicted` を送る。 |

`Vt::interpret` の契約は「VT 自身の判断による eviction は `VtSignal::WebviewEvicted` として表面化する」と定めているため、#5 のシグナル送出は省略できない。

## 既にある口（新規 API は不要）

`Screen::reset`（#2）が戻す対象と、それぞれに使うもの。

| フィールド | 戻し方 | 備考 |
| - | - | - |
| `grid` | `Grid::reset()` | **新規（#1）** |
| `viewport` | `Viewport::default()` | display offset が 0 に戻る |
| `state` | `ScreenState::default()` | カーソル位置・`pending_wrap`・SGR pen（p.331 の "Sets the select graphic rendition (SGR) function to normal rendition"、"Returns the cursor to the upper-left corner of the screen"） |
| `scroll_region` | `ScrollRegion::new(rows)` | DECSTBM マージンと DECOM を同時に戻す |
| `tabs` | `TabStops::reset()` | `Screen::reset_tab_stops` の doc に `RIS` を既に明記済み |
| `character_set_mapping` | `CharacterSetMapping::reset()` | doc に `RIS` を既に明記済み。p.331 の "Selects the default character sets" のうち GL 側 |
| `checkpoint` | `Checkpoint::default()` | `checkpoint.rs` の doc が「`DECSTR` and `RIS` reset the saved state as well」と既に宣言している |

## RIS Actions（p.331）と対応状況

| RIS Action | orzma_vt での扱い |
| - | - |
| Sets all features listed on set-up screens to their saved settings | `VtModes::default()`（#3）。アクティブスクリーンが Primary に戻るのもここに含まれる |
| Causes a communication line disconnect | 対象外。シリアル線を持たない |
| Clears user-defined keys | 対象外。DECUDK を模していない（`dcs_hook` は `todo!()`） |
| Clears the screen and all off-screen page memory | 両スクリーンの可視行とスクロールバック（#1・#3）。ページメモリは持たない |
| Clears the soft character set | 対象外。DECDLD を模していない |
| Clears page memory. All data stored in page memory is lost. | 同上。ページメモリは持たない |
| Clears the screen | #1・#3 に含まれる |
| Returns the cursor to the upper-left corner of the screen | `ScreenState::default()` |
| Sets the select graphic rendition (SGR) function to normal rendition | `ScreenState::default()` の `pen` |
| Selects the default character sets (ASCII in GL, DEC Supplemental Graphic in GR) | `CharacterSetMapping::reset()`。GR は `screen/character_sets.rs` 冒頭の決定により対象外で、DEC Supplemental Graphic 自体も未実装（`CharacterSet` の TODO） |
| Clears all macro definitions | 対象外。DECDMAC を模していない |
| Erases the paste buffer | 対象外。ペーストバッファはホスト側が持つ |

## 決定が必要な点

1. **`next_line_id` を引き継ぐか。** `Grid::new` は `next_line_id` を 0 から振り直す。`screen/grid.rs` の `LineId` の doc は「ids are minted monotonically per grid and never reused, so a placement anchored to one can never be re-pointed at later content on the same grid」と定めているため、RIS で作り直して id を再利用すると、生き残った placement の anchor が RIS 後の新しい行を指しうる。#1 で `next_line_id` を据え置くのが invariant を保つ側の選択。
2. **placement を全破棄するか。** 1 と表裏の関係にある。全破棄（#4）すれば anchor の宙吊りは起きないが、RIS を送っただけで webview が落ちることになる。`switch_screen` が「Primary の placement は alternate 表示中も破棄せず隠すだけ」としている前例に照らすと、RIS の破壊力をどこまで認めるかの判断が要る。
3. **スクロールバックを捨てるか。** p.331 は page memory の消去を明記しており、ページメモリを持たないこの端末では履歴がその位置に対応する。`Screen::erase_in_display` は「scrollback history is never touched」と doc で明言しているので、RIS だけが履歴を捨てる操作になる。
4. **消去に BCE を通すか。** `erase_in_display(All)` は pen の背景色で埋める。RIS は既定セルで埋めるべきなので、#1 のように `Cell::default()` で作り直すか、pen を先に戻してから消すかのどちらかにする。#1 に一本化すれば順序の依存自体が消える。
5. **タイトルを戻すか。** `TitleState` は空の stub だが、`VtSignal::ResetTitle` はすでに存在する。ホスト側のタイトルを RIS で戻すかは別途決める。

## 機能追加時に RIS へ結線が必要になるもの

現時点では状態そのものが無いため RIS 側にすることが無いが、実装した時点で #2 または #3 に追記が要るもの。

| 機能 | 現在地 |
| - | - |
| パレット上書き（OSC 4 / 10 / 11 / 12） | `device.rs` の `ColorTable` に TODO |
| タイトルとそのスタック（CSI 22 / 23 t） | `device.rs` の `TitleState` に TODO |
| カーソルの可視性・形状（DECTCEM / DECSCUSR） | `Screen::cursor()` が Block / steady / visible をハードコード |
| DECAWM / IRM / LNM | `device.rs` の TODO。各状態の隣に置く方針まで決まっている |
| キーパッドモード（DECKPAM / DECKPNM / DECNKM） | 未モデル。[esc-dispatch.md](esc-dispatch.md) が `VtModes` の新フィールドを要求している |
| OSC 8 ハイパーリンク | `HyperlinkInterner` は存在するが `Cell` にも `DeviceState` にも未接続 |
| 同期更新バッファ（CSI ?2026） | `interpreter.rs` の `SyncBuffer` が空の stub。RIS が同期更新の最中に届いた場合の扱いも併せて決める |

## 関連する既知の穴

対になる DECSTR（`CSI ! p`）は、`Executor::csi_dispatch` が intermediate を一律に弾く現在の方針では届かない。RIS と共有するリセット範囲を決める際は、この方針の見直しも併せて必要になる。
