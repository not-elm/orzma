# Synchronized Output と DECRQM / DECRPM

`CSI ? 2026 h/l`（同期出力）と、アプリがその有無を確かめる `CSI ? Ps $ p`（DECRQM）をまとめる。
両者は別々の制御機能だが、**2026 には terminfo の能力名が無く、アプリが存在を知る手段が DECRQM しかない**ため、
片方だけを実装しても意味が薄い。orzma は同じブランチで両方入れた。

- [Synchronized Output — contour-terminal/vt-extensions](https://github.com/contour-terminal/vt-extensions/blob/master/synchronized-output.md) — 2026 の定義
- [xterm: DECRQM / DECRPM](../references/xterm-ctlseqs.pdf#page=29) — 問い合わせと応答の定義
- 番号空間と他のモードの扱いは[DECSET / DECRST](decset.md)を参照

## 制御関数一覧

| 制御関数 | 7-bit形式 | 7-bitバイト列 | 説明 |
| - | - | - | - |
| DECSET 2026 | `CSI ? 2 0 2 6 h` | `1B 5B 3F 32 30 32 36 68` | 同期更新を開く。以後 frame の送出を止める。 |
| DECRST 2026 | `CSI ? 2 0 2 6 l` | `1B 5B 3F 32 30 32 36 6C` | 同期更新を閉じる。閉じた瞬間の画面を frame にする。 |
| DECRQM（DEC private） | `CSI ? Ps $ p` | `1B 5B 3F {Ps} 24 70` | DEC private モード `Ps` の状態を問い合わせる。 |
| DECRQM（ANSI） | `CSI Ps $ p` | `1B 5B {Ps} 24 70` | ANSI モード `Ps` の状態を問い合わせる。 |
| DECRPM（DEC private） | `CSI ? Ps ; Pm $ y` | `1B 5B 3F {Ps} 3B {Pm} 24 79` | DEC private DECRQM への応答。 |
| DECRPM（ANSI） | `CSI Ps ; Pm $ y` | `1B 5B {Ps} 3B {Pm} 24 79` | ANSI DECRQM への応答。 |

CSI は 8-bit 形式（`9B`）でも同じに扱う。`\x9b?2026h` は `\x1b[?2026h` と等価。

## 同期出力（`CSI ? 2026 h/l`）

### 何のためのモードか

1 回の再描画が複数の PTY チャンクに分かれると、端末は描きかけの画面を表示してしまう（ティアリング）。
アプリは描画の前後を 2026 で囲み、「閉じるまで画面を更新するな」と伝える。fzf はフレームごとに発行し、
nvim・tmux・kitty も使う。

### 止まるのは frame だけ

| 止まるもの | 止まらないもの |
| - | - |
| `TtyFrame` の送出（＝GUI への画面反映） | パース。バイトは通常どおり解釈する |
| | DSR などの応答。アプリへの返信は遅れない |
| | `VtSignal` / `TtySignal`。PTY が上げた signal はそのまま流れる |

signal が流れて frame だけ止まるので、**signal が指す画面の変化は遅れて見える**。下の「抑止の代償」を参照。

### 層ごとの役割

```
orzma_vt   VtModes::synchronized_output に Active/Inactive を持つだけ。
           Interpreter::parse は「閉じたバイト」で走査を止め、consumed と
           synchronized_update_closed を返す。
             ↓ InterpretOutput
orzma_tty  OrzmaTty が締め切り（sync_deadline）と間引きを持つ。frame を
           出すかどうかを決めるのはこの層だけ。残りのバイトは feed_chunk が
           chunk[consumed..] として再投入する。
             ↓ PumpOutput（signal と frame が 1 本の順序付きリスト）
orzmux     backend は pane ごとに next_deadline を見て pump する。2026 を
           知らない。
```

閉じたところで `parse` を止めるのは、**閉じた瞬間の画面**を frame にするため。チャンクの末尾まで解釈してしまうと、
次の同期更新の描きかけまで混ざる。`consumed == 0` や `consumed > len` を返す VT 実装は契約違反で、`OrzmaTty::feed_chunk` が
`OrzmaTtyError::VtConsumedNothing` / `VtConsumedBeyondChunk` を返す。回復できない境界である
`drain_chunks` がそれを警告し、そのチャンクの残りを捨てる。

### べき等とリセット

| 操作 | 結果 |
| - | - |
| 開いている間の `?2026h` | no-op。締め切りも延長しない |
| 開いていないときの `?2026l` | no-op。`parse` も止まらない |
| RIS（`ESC c`） | 閉じる。`VtModes` が既定値に戻るため |
| DECSTR（`CSI ! p`） | 閉じない。soft reset は `synchronized_output` に触らない |
| `CSI ? 2026 ; 25 l` のような複合指定 | スロットを全部適用してから、その最終バイトで止まる |

### タイムアウト — 150 ms

アプリが閉じ忘れる（クラッシュ、SIGSTOP、開いたまま長考）と画面が永久に凍るので、締め切りを置く。

- `OrzmaTty::SYNC_TIMEOUT` = 150 ms。締め切りは**最初の `?2026h` に固定**し、再度の `?2026h` では延長しない。
- 過ぎたら描画だけ再開する。`VtModes` のフラグには触らないので、**DECRQM はアプリが set した値（1）を返し続ける**。
  端末が勝手に reset したことにすると、アプリから見て自分が送っていない状態遷移が起きる。
- `OrzmaTty::next_deadline` は、抑止中はこの締め切りを返す。orzmux の `select` はそれで起こされる。

### 閉じた瞬間の frame の間引き — 12 ms

close のたびに frame を取ると、fzf のようにフレームごとに開閉するアプリでは GPU 側が溢れる。

- 直前の emit から `OrzmaTty::SYNC_EMIT_INTERVAL` = 12 ms 未満なら、その close では frame を取らない。
- damage は残るので、通常の coalesce 窓（`Coalescer::IDLE` = 3 ms / `Coalescer::MAX_CAP` = 12 ms）で拾われる。
- ただし次の同期更新がその窓より先に開くと、次の close かタイムアウトまで待つ。タイムアウト後の描きかけ frame も
  間引きの基準に入るので、1 回の更新が 150 ms を超える出力が続く間は、完成した画面が出ないことがある。
- **1 つの frame が留まるのは最大で 12 + 150 ms**。

### 抑止を貫通するもの

`OrzmaTty::flush_now` だけ。orzmux が呼ぶのは resize 直後と pane を閉じるときの 2 か所で、同期更新は開いたままにする。
bootstrap の最初の frame も、ユーザーのスクロールと選択の描画も抑止に従う。

### 抑止の代償

- control socket 経由の mount の signal は、出力の無い pane では最大 150 ms 遅れる。
- webview の unmount / evict の signal は即座に届くが、それを反映する frame は抑止される。アプリが close を送る前に
  止まると、webview が消えた跡に古いセルが見える。

## DECRQM / DECRPM

### 形式

`CSI ? Ps $ p`（DEC private 形）と `CSI Ps $ p`（ANSI 形）に、`CSI ? Ps ; Pm $ y` / `CSI Ps ; Pm $ y` で答える。
`Pm` の意味は xterm-ctlseqs.pdf p.29 のとおり。

| `Pm` | 意味 | orzma |
| - | - | - |
| 0 | not recognized | 使う |
| 1 | set | 使う |
| 2 | reset | 使う |
| 3 | permanently set | 使わない |
| 4 | permanently reset | 使わない |

**未実装のモードに 0 で答えられる**ので、アプリ側の機能検出が成立する。これが 2026 の広告手段になる。

### 応答表

`DeviceState::private_mode_report` / `DeviceState::ansi_mode_report` が引く表。

| 種別 | `Ps` | 応答 |
| - | - | - |
| DEC | 1, 7, 25, 66, 1004, 1007, 2004, 2026 | 対応する `VtModes` のフィールドに応じて 1 / 2 |
| DEC | 6 | 表示中の画面の origin mode に応じて 1 / 2 |
| DEC | 12 | `text_cursor.blink` に応じて 1 / 2。DECSCUSR が変えた点滅も反映する |
| DEC | 47, 1047, 1049 | 3 つとも「代替画面を表示中か」 |
| DEC | 1000, 1002, 1003 | 現在のトラッキングレベルと一致すれば 1、しなければ 2 |
| DEC | 1006 | 現在のエンコーディングが SGR なら 1 |
| DEC | 3, 40, 95, 1005, 1034, 1048, その他 | 0 |
| ANSI | 4（IRM） | 状態に応じて 1 / 2 |
| ANSI | その他 | 0 |

電源投入時に 1 を返すのは 7（DECAWM）・25（DECTCEM）・1007（Alternate Scroll）。2026 は 2 を返す。

### パラメータの扱い

| 入力 | 応答 |
| - | - |
| `CSI ? 2026 $ p` | `CSI ? 2026 ; 2 $ y` |
| `CSI ? 99999 $ p` | 表は `u16` に飽和した値で引くが、**エコーは受け取った整数をそのまま書く** |
| `CSI ? $ p`（省略） | `Ps = 0` として `CSI ? 0 ; 0 $ y` |
| 2 スロット目以降 | 見ない。先頭スロットだけを答える |

### 2026 との関係

- 2026 を実装しても DECRQM が無ければ、先に問い合わせるアプリ（tmux は pane を作った直後に投げる）からは
  未実装と区別が付かない。
- タイムアウトで描画が再開したあとも、アプリが reset するまで 1 を返す（上の「タイムアウト」を参照）。

### 不変条件 — 状態を変える private モードは 0 を返してはならない

`CSI ? Ps h/l` が `VtModes`（または origin mode）を動かすのに DECRQM が 0（未認識）を返すと、アプリは
「その番号は効かない」と判断したうえで効いてしまう状態に置かれる。`orzma_vt` はこれを総当たりのテスト
（`interpreter::tests::mode_report::every_stateful_private_mode_is_reported`、0〜2100 を全部叩く）で縛っている。
**新しい private モードを `set_private_modes` に足すときは、同時に応答表へ足すこと。**

## 注意点

### DECRQM は damage を上げない

問い合わせは画面を変えないので、`?2026$p` も `?2026h` / `?2026l` 自体も liveness を上げない。応答バイトだけが
PTY へ流れる。frame の有無は damage で決まるので、問い合わせだけの pane は描画を起こさない。

### 3 / 40 / 95 に 0 を返すのは alacritty 寄せ

xterm はこれらに 2 を返す。orzma は「受理しても状態を持たないモードには 0 で答える」という規則で揃えている
（1034 も同じ。1005 はどのマウスモードにも該当せず無視される。1048 は状態の無い操作）。

### 読み取り専用モードは持たない

xterm には `13` / `14`（cursorBlink 系）や `1020`〜`1023` のように、報告のためだけに存在する private モードがある。
orzma はどれも実装していないので 0 を返す。
