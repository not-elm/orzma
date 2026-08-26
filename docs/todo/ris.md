# RIS

RIS（`ESC c`、`1B 63`）は端末を初期状態へ戻す制御関数であり、terminfo の `rs1=\Ec` として `reset(1)` や `tput reset` が送出する。実装の優先度と位置づけは [esc-dispatch.md](esc-dispatch.md) に記したとおりで、ここでは実装に必要な API を洗い出す。

一次情報は [vt510](../references/vt510.pdf) p.331 の RIS Actions を用いる。ESC と C1 の対応そのものは [esc.md](../memo/esc.md) にまとめてある。

## 追加が必要な API

| # | 追加先 | シグネチャ | 内容 |
| - | - | - | - |
| 1 | `screen/grid.rs` | `pub fn reset(&mut self)` / `pub fn is_blank(&self) -> bool` | **実装済み。** 履歴を捨て、可視行を `Cell::default()` で作り直す。`size` と `max_history` は据え置き。作り直す各行の id は `mint()` で採番するので、`next_line_id` は引き継いだまま前進し、reset 前のどの id とも一致しない（決定事項 1）。`is_blank()` は #2 の述語用で、履歴が無く全可視セルが `Cell::default()` かだけを見る — 行 id にも `next_line_id` にも触れない。 |
| 2 | `screen.rs` | `pub fn reset(&mut self) -> Option<DamageSpan>` | **実装済み。** `Screen` の全7フィールドを初期状態へ戻す。内訳は次節の表のとおり。返り値は `Some(DamageSpan::Full)`、ただし **reset 前の grid が空で履歴も無ければ `None`**（`Option<DamageSpan>` は「何も起きなかったか」ではなく「再描画が要るか」を報告する値で、カーソル移動は per-chunk cursor diff で届くため）。`grid.reset()` は画面が空でも常に通すので、reset を跨いで placement anchor が生き残ることは無い。 |
| 3 | `device.rs` | `pub fn reset(&mut self) -> Option<DamageSpan>` | **実装済み。** 両スクリーンへ #2 を適用し、`modes` を `VtModes::default()` へ戻す。**前提が変わった**: #2 が `None` を返しうるようになったので、「RIS は必ず全面を書き換える」は成り立たず、`Option` を畳めない。フレームに出るのは *リセット後にアクティブな* Primary の damage だけなので、`Some(DamageSpan::Full)` を返すのは (a) Primary 側の #2 が damage を報告したとき、または (b) リセット前に Alternate がアクティブだったとき（`VtModes::default()` による暗黙のスクリーン切替そのものが全面書き換え）に絞る。Alternate 側の #2 が返す damage はフレームに出ないので握り潰す。`active()` / `active_mut()` しか無いので非アクティブ側へ届く口が現状は無いが、`screens` フィールドを持つ `DeviceState` 自身の実装であれば新しい到達手段は要らない。 |
| 4 | `interpreter.rs` | `Executor::esc_dispatch` の `(b'c', [])` アーム | **実装済み。** #3 を呼び、その返り値を既存の `Executor::stage` に渡すだけ（`DamageSpan::Full` 固定ではなくなった）。RIS は C1 形式を持たないので、`print` と同じ 2 行をアームへ直接置き、C1 と ESC の対応を守る `impl Executor` ブロックには入れない。placement の破棄に専用の呼び出しは要らない。[placement-ownership-design.md](../superpowers/specs/2026-08-26-placement-ownership-design.md) の「帰結 — `Screen::reset` は placement について何も返さない」節が示すとおり、`#2`（`Screen::reset`）が呼ぶ `Grid::reset` は行 id を振り直すので RIS 前のどの anchor も解決不能になり、あとはチャンク末尾の `DeviceState::evict_lost_anchors`（両スクリーン sweep）が通常どおり回収する。handler 側が全消し専用の API や signal 送出を持つ必要はない。 |

`Vt::interpret` の契約は「VT 自身の判断による eviction は `VtSignal::WebviewEvicted` として表面化する」と定めているが、これを満たすのはチャンク末尾の両スクリーン sweep（`Executor` が `evict_lost_anchors` の結果を送出する箇所、まだ `todo!()`）であって RIS ハンドラ個別の仕事ではない。したがって #4 のハンドラ自体はシグナルを送らない。

## 既にある口（新規 API は不要）

`Screen::reset`（#2）が戻す対象と、それぞれに使うもの。

| フィールド | 戻し方 | 備考 |
| - | - | - |
| `grid` | `Grid::reset()` | **新規（#1）、実装済み** |
| `viewport` | `Viewport::default()` | display offset が 0 に戻る |
| `state` | `ScreenState::default()` | カーソル位置・`pending_wrap`・SGR pen（p.331 の "Sets the select graphic rendition (SGR) function to normal rendition"、"Returns the cursor to the upper-left corner of the screen"）。origin mode を参照せず直接 `(0, 0)` を座らせるので、`scroll_region` との適用順に依存しない |
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

1. ~~**`next_line_id` を引き継ぐか。**~~ **決定済み（引き継ぐ）。#1 で実装。** `Grid::grid_line` は `rows.iter().rposition(|row| row.id == id)` の素なリニアスキャンで、`LineId` の doc が唯一健全と呼ぶ `id >= next_line_id` のガードは実装されていない。したがって counter を巻き戻すと、生き残った anchor が reset 後の新しい行に**一致してしまい**、`project_placements` が無関係な行に描画し、`evict_lost_anchors`（対応する `Grid::grid_line` が `None` を返すときだけ evict する）も発火しない。
   なお「counter を据え置く」だけでは足りず、**作り直す行を据え置いた counter から新規採番する**ところまでが一組になる（`Grid::new` で作り直してから counter だけ戻す実装だと、新しい行が 0 から振られて同じ衝突が再現する）。
   外部の前例も同じ向き: wezterm の `StableRowIndex` は `stable_row_index_offset` が `scroll_up` と `erase_scrollback` で前進するのみで、`full_reset` も RIS も巻き戻さない。xterm.js の `Marker._nextId` は class static で reset を跨いでもリセットされない（marker 側は全破棄する）。alacritty と kitty は行 identity 自体を持たない。identity を持つ端末で counter を巻き戻す前例は見つからなかった。
   u64 枯渇は検討に値しない: 採番は実際にスクロールが起きた 1 行につき 1 個で、10k 行/秒でも約 5,800 万年。`Grid::mint` の `checked_add(...).expect(...)` が既に正しい姿勢。
   回帰は `screen/grid.rs` の `mod tests::reset` の 2 件（`a_reset_mints_ids_no_pre_reset_anchor_can_match` と `a_reset_does_not_rewind_the_id_counter`）が押さえている。
2. ~~**placement を全破棄するか。**~~ **決定済み（破棄する）。** 1 と表裏の関係にある。当初案は専用の `pub(crate) fn clear(&mut self) -> Vec<PlacementId>` API で全破棄する形だったが、決定事項 1（`next_line_id` を巻き戻さない）と eviction 経路の両スクリーン sweep への一本化により、専用 API 無しで同じ結果が出るとわかった。`Grid::reset` が id を振り直すことで RIS 前のどの anchor も解決不能になり、チャンク末尾の `evict_lost_anchors` が両スクリーンぶん回収する。つまり RIS は放置すれば全 webview が落ちる、という結論自体は変わらないが、その根拠は「専用の全消し API を呼ぶ」から「sweep が通常どおり動いた結果として全部落ちる」に変わった。`switch_screen` が「Primary の placement は alternate 表示中も破棄せず隠すだけ」としている前例と対照的だが、RIS はその温存を継承しない。詳細は [placement-ownership-design.md](../superpowers/specs/2026-08-26-placement-ownership-design.md) の「帰結」節を参照。
3. ~~**スクロールバックを捨てるか。**~~ **決定済み（捨てる）。#1・#2 で実装。** p.331 は page memory の消去を明記しており（"Clears the screen and all off-screen page memory"、"Clears page memory. All data stored in page memory is lost."）、ページメモリを持たないこの端末では履歴がその位置に対応する。`Screen::erase_in_display` は「scrollback history is never touched」と doc で明言しているので、RIS だけが履歴を捨てる操作になる。`display_offset` も 0 に戻る（履歴が消えた後にスクロール位置を保つ意味が無く、`viewport_row` が範囲外を読む）。
4. ~~**消去に BCE を通すか。**~~ **決定済み（通さない）。#1 に一本化して実装。** `erase_in_display(All)` は pen の背景色で埋めるが、RIS は `Grid::reset` が `Cell::default()` で作り直すので pen を持ち込まない。順序の依存自体が消えた。BCE は vt220 / vt510 / ECMA-48 のいずれにも記述が無く（terminfo の `bce` capability 由来）、vt220 p.36 の "Erasing a character also erases any character attribute of the character" はむしろ逆を向いている。
5. ~~**タイトルを戻すか。** `TitleState` は空の stub だが、`VtSignal::ResetTitle` はすでに存在する。ホスト側のタイトルを RIS で戻すかは別途決める。~~ 戻さない

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
