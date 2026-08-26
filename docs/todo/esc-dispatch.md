# ESC Dispatch

ここでは `orzma_vt` の `Executor::esc_dispatch` が受け持つ制御関数を、実装済み・実装予定・保留・対象外に分けてメモする。

一次情報は [VT510 Video Terminal Programmer Information](../references/vt510.pdf)（以下 vt510）を用い、実装の当たりを取るために alacritty（`vte 0.15` の `ansi.rs`）と terminfo `xterm-256color` を参照する。ESC と C1 の対応そのものは [control-function.md](control-function.md) に、文字集合の指示・呼出しは [character-set.md](character-set.md) にまとめてある。

## esc_dispatch の担当範囲

`docs/memo/control-function.md` の ESC↔C1 対応表のうち、`esc_dispatch` に届くのは一部だけである。`vtparse` の `escape()` 遷移表が、文字列を導入するバイトを専用のステートへ振り分けるためである。

| バイト | 制御関数 | vtparse の遷移先 | esc_dispatch に届くか |
| - | - | - | - |
| `ESC [` | CSI | `CsiEntry` | 届かない |
| `ESC ]` | OSC | `OscString` | 届かない |
| `ESC P` | DCS | `DcsEntry` | 届かない |
| `ESC X` | SOS | `SosPmString` | 届かない |
| `ESC ^` | PM | `SosPmString` | 届かない |
| `ESC _` | APC | `ApcString` | 届かない |
| `ESC \` | ST | `EscDispatch` | 届く |

したがって CSI・OSC・DCS・APC・PM・SOS の導入子は `esc_dispatch` の対象外であり、それぞれ `csi_dispatch`・`osc_dispatch`・`dcs_hook`・`apc_dispatch` が受け持つ。`ESC \` だけは Ground にいる間に現れると `esc_dispatch` へ届く。

なお intermediate を伴う形式（`ESC # 8`、`ESC SP F`、`ESC ( Dscs` など）は、`0x20`–`0x2F` を `EscapeIntermediate` ステートで収集してから final byte で `esc_dispatch` へ渡される。

## 実装済み

| 制御関数 | 7-bit形式 | 7-bitバイト列 | 説明 |
| - | - | - | - |
| DECSC | `ESC 7` | `1B 37` | カーソル位置と関連する状態を保存する。 |
| DECRC | `ESC 8` | `1B 38` | DECSC で保存した状態を復元する。 |
| IND | `ESC D` | `1B 44` | カーソルを同じ列のまま1行下へ移動する。下マージン上ではマージン内を上へスクロールする。 |
| NEL | `ESC E` | `1B 45` | 復帰してからカーソルを1行下へ移動する。 |
| HTS | `ESC H` | `1B 48` | カーソル位置に水平タブ停止位置を設定する。 |
| RI | `ESC M` | `1B 4D` | カーソルを同じ列のまま1行上へ移動する。上マージン上ではマージン内を下へスクロールする。 |
| SS2 | `ESC N` | `1B 4E` | 次の1図形文字だけ G2 を GL へ呼び出す。 |
| SS3 | `ESC O` | `1B 4F` | 次の1図形文字だけ G3 を GL へ呼び出す。 |
| RIS | `ESC c` | `1B 63` | 端末を初期状態へ戻す。両スクリーンとスクロールバックを消去し、モード・マージン・タブ停止位置・文字集合・SGR・カーソル位置を初期化する。詳細は [ris.md](ris.md)。 |
| LS2 | `ESC n` | `1B 6E` | G2 を GL へ呼び出す。 |
| LS3 | `ESC o` | `1B 6F` | G3 を GL へ呼び出す。 |
| SCS | `ESC ( Dscs` | `1B 28 {Dscs}` | 94文字集合を G0 へ指示する。 |
| SCS | `ESC ) Dscs` | `1B 29 {Dscs}` | 94文字集合を G1 へ指示する。 |
| SCS | `ESC * Dscs` | `1B 2A {Dscs}` | 94文字集合を G2 へ指示する。 |
| SCS | `ESC + Dscs` | `1B 2B {Dscs}` | 94文字集合を G3 へ指示する。 |

## 実装予定

実プログラムが日常的に送出するもの、および実装費用が低く検証に効くものを対象とする。

### 優先度 高

terminfo `xterm-256color` の初期化・終了文字列に含まれており、ncurses を使うアプリケーションが起動と終了のたびに送出する。

| 制御関数 | 7-bit形式 | 7-bitバイト列 | 説明 | vt510 |
| - | - | - | - | - |
| DECKPAM | `ESC =` | `1B 3D` | 補助キーパッドをアプリケーションモードにする。 | p.94 |
| DECKPNM | `ESC >` | `1B 3E` | 補助キーパッドを数値モードに戻す。 | p.95 |

DECKPAM / DECKPNM は terminfo の `smkx=\E[?1h\E=` と `rmkx=\E[?1l\E>` に対応する。さらに `ESC >` は初期化文字列 `is2` と `rs2` にも含まれるため、送出頻度は RIS より高い。

alacritty はどちらも実装している。

### 優先度 中

| 制御関数 | 7-bit形式 | 7-bitバイト列 | 説明 | vt510 |
| - | - | - | - | - |
| ST | `ESC \` | `1B 5C` | 文字列を終端する。Ground で単独に現れた場合は何もしない。 | - |
| DECALN | `ESC # 8` | `1B 23 38` | 画面調整用の試験パターンで画面全体を埋める。 | p.124 |

ST は現在も `_ => {}` が拾って無視するため挙動は変わらない。明示的なアームは、無視が意図的な決定であることを記録するために置く。alacritty も同じ理由で明示的な no-op を持つ。

DECALN は vt510 p.124 によると、マージンをページの両端へ設定し、カーソルをホーム位置へ移動したうえで画面を埋める。通常のアプリケーションは送出しないが、vttest による適合性検証で使う。

### 実装に必要な新しい API

| 制御関数 | 必要なもの |
| - | - |
| DECKPAM / DECKPNM | `VtModes` の補助キーパッドモードを表すフィールド。DECSET 66（DECNKM）と同じ状態を指すため、両者で共有する。 |
| DECALN | 画面全体を1文字で埋める `Screen` のメソッド。マージンのリセットとカーソルのホーム復帰を伴う。 |
| ST | なし。 |

## 保留

先に別の機能が必要なため、実装予定には含めない。

| 制御関数 | 7-bit形式 | 7-bitバイト列 | 説明 | 先に必要なもの |
| - | - | - | - | - |
| DECID | `ESC Z` | `1B 5A` | 端末が装置属性を応答する。 | 応答の送出経路。vt510 p.89 は VT510 では非対応とし、DA1 の使用を勧めている。 |
| S7C1T | `ESC SP F` | `1B 20 46` | 応答の C1 を7ビット形式で送出する。 | 応答の送出経路。 |
| S8C1T | `ESC SP G` | `1B 20 47` | 応答の C1 を8ビット形式で送出する。 | 応答の送出経路。 |
| DECDHLT | `ESC # 3` | `1B 23 33` | 現在行を倍幅倍高の上半分にする。 | 行単位の幅・高さ属性とレンダラ側の対応。 |
| DECDHLB | `ESC # 4` | `1B 23 34` | 現在行を倍幅倍高の下半分にする。 | 同上。 |
| DECSWL | `ESC # 5` | `1B 23 35` | 現在行を等幅等高に戻す。 | 同上。 |
| DECDWL | `ESC # 6` | `1B 23 36` | 現在行を倍幅等高にする。 | 同上。 |
| DECBI | `ESC 6` | `1B 36` | カーソルを1列左へ移動する。左マージン上ではマージン内のデータを右へ1列移動する。 | 左右マージン。 |
| DECFI | `ESC 9` | `1B 39` | カーソルを1列右へ移動する。右マージン上ではマージン内のデータを左へ1列移動する。 | 同上。 |
| SPA | `ESC V` | `1B 56` | 保護領域を開始する。 | DECSCA の保護属性。 |
| EPA | `ESC W` | `1B 57` | 保護領域を終了する。 | 同上。 |

倍幅倍高の4つと DECBI / DECFI は alacritty も実装していない。

## 対象外

実装しないことが既に決まっているもの。

| 制御関数 | 7-bit形式 | 7-bitバイト列 | 対象外とする理由 |
| - | - | - | - |
| LS1R | `ESC ~` | `1B 7E` | GR を模していないため。`screen/character_sets.rs` の冒頭に記録があるとおり、GR へ呼び出した集合へ到達するには `0xA0`–`0xFF` の生バイト入力が要るが、この crate が読む UTF-8 の並びはその範囲を多バイト符号として消費する。 |
| LS2R | `ESC }` | `1B 7D` | 同上。 |
| LS3R | `ESC \|` | `1B 7C` | 同上。 |
| SCS（96文字集合） | `ESC - Dscs` 他 | `1B 2D {Dscs}` 他 | 96文字集合を扱わないため。[character-set.md](character-set.md) の決定に従う。vt510 p.339 は `-` `.` `/` を G1〜G3 への96文字集合の指示子としている。 |
| DECANM | `ESC <` | `1B 3C` | VT52 モードを持たないため。 |
| memory lock / unlock | `ESC l` / `ESC m` | `1B 6C` / `1B 6D` | xterm 独自の拡張であり、実質的に使われていないため。 |

## 別途対応する既知の穴

vt510 p.339 の Dscs には `%5`（DEC Supplemental）、`"?`（DEC Greek）、`&4`（DEC Cyrillic）のように2バイトからなるものがある。これらは `ESC ( % 5` のように intermediate が2つ収集された状態で `esc_dispatch` へ届くため、intermediate を1つだけ受ける現在の SCS のアームにマッチしない。結果として、未対応の指示子に対する ASCII へのフォールバックも行われず、シーケンス全体が無視される。

これは `an_unsupported_designation_falls_back_to_ascii` が守っている方針と食い違うが、上記の実装予定とは独立した課題である。
