# `DeviceState::resize` の関連仕様

`crates/orzma_vt/src/device.rs:89` の `todo!()` を実装するにあたり、
`docs/references/vt510.pdf` と `docs/references/xterm-ctlseqs.pdf` から
拘束される仕様を洗い出したもの。ページ番号は PDF の物理ページ。

## 要点

VT510 には**幅を変える制御機能が2つ**あり、副作用だけが違う。

| | ページメモリ | スクロールマージン |
| --- | --- | --- |
| **DECCOLM**（`CSI ? 3 h/l`、p.143） | **全消去** | **既定位置にリセット** |
| **DECSCPP**（`CSI Ps $ \|`、p.249） | 消さない | 変えない |

> It is recommended that new applications use DECSCPP rather than DECCOLM.
> DECSCPP does not clear page memory or reset the scrolling regions, as does
> DECCOLM. DECCOLM is provided mainly for compatibility with previous products.
> — VT510 p.143 / p.249

**ウィンドウのリサイズは意味論的に DECSCPP であって DECCOLM ではない。**
`DeviceState::resize` が従うべきなのは DECSCPP 側の規定になる。

---

## 1. 3つのサイズ概念

VT510 はサイズを3つに分けており、これが `orzma_vt` の型に対応する。

| VT510 | 意味 | 対応する型 |
| --- | --- | --- |
| Lines per **page**（DECSLPP、p.260） | ページメモリの行数 | `Grid` の rows |
| Lines per **screen**（DECSNLS、p.264） | 画面に表示できる行数 | `Viewport` |
| Columns per page（DECSCPP / DECCOLM） | ページの桁数 | `Grid` の cols |

> The page size determines the addressing range for cursor positioning and
> scroll regions. — VT510 p.32 §2.5.2

つまり**カーソルのアドレス範囲とスクロール領域を決めるのは page（= `Grid`）の
方**で、screen（= `Viewport`）ではない。

page > screen のとき、カーソル追従でウィンドウがパンする挙動が DECVCCM
（`CSI ? 61 h/l`、p.293）として規定されている。ただしこれは**カーソル移動に
よるパン**の規定であって、リサイズ時のビューポートをどう置くかは規定していない。

---

## 2. resize を直接拘束する規定

### 2-1. カーソルのクランプ（DECSCPP、p.249）

> DECSCPP does not move the cursor. If, however, the cursor is beyond the width
> of the new page when DECSCPP executes, then the cursor moves to the right
> column of the new page.

- 幅が変わってもカーソルは**動かさない**
- ただし新しい幅を超えている場合のみ、**右端の列**へ移す

行方向について同等の規定は DECSLPP に無い。

### 2-2. 狭めたときはリフローせず切り詰める（DECSCPP p.249、§2.5.3 p.32）

> If you switch from 132-column to 80-column pages, then you can lose data from
> page memory. Columns no longer present in page memory are lost. — p.249

> changing this feature does not clear page memory, except when changing from
> 132 columns to 80 columns; then, columns 81 through 132 of each page are
> cleared. — p.32 §2.5.3

VT510 は**切り詰める**。折り返し直しはしない。「of each page」とあるので、
可視行だけでなくページメモリ上の全行が対象。

### 2-3. 高さ変更時のマージン（DECSLPP、p.260）

> DECSLPP usually does not change the top and bottom scrolling margins. If,
> however, you change the page size so that the current scrolling margins exceed
> the new page size, then the terminal resets the margins to the page limits.

- 原則としてマージンは**保つ**
- 新しい高さに収まらなくなった場合のみ、**ページ全体にリセット**する

「reset to the page limits」であって「clamp」ではない点に注意。
下マージンだけを新しい最終行に詰めるのではなく、上下とも既定に戻る。

あわせて DECSTBM（p.276）の既定値:

> Pb — Default: Pb = current number of lines per screen.

マージンが既定のままなら、新しい高さに追随する。

### 2-4. 高さを縮めたときのデータ（DECSLPP、p.260）

> If you switch to a smaller page size, then data that was on the larger page
> may be split across the smaller pages.

VT510 固有のページング概念なのでそのままは適用できないが、**仕様が内容の分断を
許容している**ことは読み取れる。

### 2-5. 代替画面は必ず表示領域と同じ大きさ（xterm p.52）

> The Alternate Screen Buffer is exactly as large as the display, contains no
> additional saved lines. When the Alternate Screen Buffer is active, you cannot
> scroll back to view saved lines.

代替画面のリサイズは、あふれた行を履歴へ逃がせない。**失うしかない。**
`DeviceState::resize` は両画面をリサイズするので、主画面と代替画面で
あふれ行の扱いを変える必要がある。

### 2-6. リサイズの入口となるシーケンス（xterm p.31–32）

| シーケンス | 意味 |
| --- | --- |
| `CSI 8 ; height ; width t` | テキスト領域を文字数で変更。省略パラメータは現在値、`0` は画面サイズ |
| `CSI Ps t`（`Ps >= 24`） | Ps 行にリサイズ（DECSLPP） |
| `CSI 18 t` | テキスト領域サイズの報告 → `CSI 8 ; height ; width t` |
| `CSI 19 t` | 画面サイズの報告 → `CSI 9 ; height ; width t` |

`CSI 18 t` / `CSI 19 t` の報告値はリサイズ後の値を返す必要がある。

### 2-7. DECNCSM（`CSI ? 95 h/l`、p.190）

> When enabled, a column mode change (either through Set-Up or by the escape
> sequence DECCOLM) does not clear the screen. When disabled, the column mode
> change clears the screen as a side effect.
> This sequence does not affect the column mode change caused by the sequence,
> DECSCPP.

DECCOLM の画面消去を抑止するモード。**DECSCPP には影響しない**と明記されている
ので、ウィンドウリサイズ経路には効かない。

---

## 3. 仕様が決めていないこと

実装時に**自分で決める必要がある**箇所。仕様に照らす先が無いので、
判断とその理由を doc コメントに残すべきところ。

### 3-1. リフロー / 折り返し直し

**VT510 にも xterm ctlseqs にも記述が無い**（`xterm-ctlseqs` を
`rewrap` / `reflow` / `re-wrap` で検索して0件）。VT510 の唯一の答えは
2-2 の「切り詰める」。

`device.rs` の `// TODO:` が挙げている placement アンカーの行再割り当ては、
**準拠すべき仕様が存在しない完全な実装定義領域**。したがってこの TODO は
「仕様を調べれば解ける」性質のものではなく、設計判断そのもの。

### 3-2. 画面を縮めたときのスクロールバックへの押し出し

近代的な端末は消える行を履歴へ送るが、**VT510 にはスクロールバックの概念が
無い**（`Viewport` / `DisplayOffset` に相当するものは「user window」だけで、
これはページメモリ内の窓であって履歴ではない）。xterm 側も `saveLines` を
リソースとして述べるだけで、リサイズ時の挙動は規定していない。

### 3-3. リサイズ後のビューポート位置

DECVCCM（p.293）はカーソル移動に伴うパンの規定であって、リサイズ時に
`DisplayOffset` をどこへ置くかは決めていない。

### 3-4. 保存カーソル（DECSC）が範囲外になったとき

DECSC が保存する項目は VT510 p.243 に列挙されている（カーソル位置、SGR、
G0–G3 と GL/GR、ラップフラグ、DECOM、選択消去属性、SS2/SS3）。しかし
**リサイズで保存位置が範囲外になった場合の規定は無い。**

`Checkpoint`（`screen/checkpoint.rs:29`）は `line` / `column` を持つので、
2-1 のカーソルクランプを保存カーソルにも適用するかどうかは自分で決める。

### 3-5. 幅変更時のタブストップ

VT510 は何も述べていない（DECST8C は p.275、TBC は 8 桁ごとの再設置と全消去
だけを規定）。

ただし `orzma_vt` では**既に決着済み**。`TabStops`（`screen/tabs.rs`）は
4096 桁固定の表で、doc に理由が書かれている:

> The table spans more columns than any grid, so a resize never has to decide
> what the columns it widens into should contain.

---

## 4. `Screen` の8フィールドへの対応

| フィールド | 拘束する仕様 |
| --- | --- |
| `grid` | 2-2 切り詰め（DECSCPP）。2-5 で主画面/代替画面の差 |
| `viewport` | **規定なし**（3-3） |
| `state`（カーソル） | 2-1 列クランプ（DECSCPP）。行方向は規定なし |
| `scroll_region` | 2-3 マージン保持とリセット（DECSLPP）、2-4 既定値追従（DECSTBM） |
| `tabs` | 規定なしだが実装側で決着済み（3-5） |
| `character_set_mapping` | 影響なし |
| `checkpoint` | **規定なし**（3-4） |
| `placements` | **規定なし**（orzma 固有） |

---

## 5. 参考: リサイズを伴わないリセット系との対比

`DECSTR`（ソフトリセット）の既定値表は VT510 p.277（Table 5–9）。
DECSTBM は "Top margin = 1; bottom margin = page length" に戻る。
`DeviceState::reset` は実装済みなので、resize がマージンをリセットする条件
（2-3）は DECSTR のそれとは別物である点に注意。
