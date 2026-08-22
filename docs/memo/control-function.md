# ESC

ECMA-48: 8.3.48 ESC - ESCAPE

ESCはC0の一種であり、制御関数を拡張するために使用される。
`ESC(0x1B)`から始まるその命令を表すシーケンスはエスケープシーケンスと呼称され、命令によって細部の構造が異なる。

また、C1は8ビットで制御関数を表すが、ESCを使って７ビット環境向けに同じ関数を提供することができる。
UTF8と競合が発生しないなどの理由から現代でもデフォルトでESCを利用されることが多い。[ctrlseqs](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)によるとS8C1Tが1の時8ビットで

## 7bit形式(ESC)と8bit形式(C1)

ESCの中にはC1の制御関数を7ビット環境で再現するために各C1に対応した命令が用意されている。
<!-- 形式の指定にはS7C1T(Select 7-bit C1 Control Transmission)とS8C1T(Select 8-bit C1 Control Transmission)が利用される。 -->

| 制御関数 | 説明 | 7bit形式 | 7bitバイト列 | 8bit形式(C1) |
| --- | --- | --- | --- | --- |
| IND | Index | `ESC D` | `1B 44` | `84` |
| NEL | Next Line | `ESC E` | `1B 45` | `85` |
| HTS | Tab Set | `ESC H` | `1B 48` | `88` |
| RI | Reverse Index | `ESC M` | `1B 4D` | `8D` |
| SS2 | Single Shift Select of G2 Character Set | `ESC N` | `1B 4E` | `8E` |
| SS3 | Single Shift Select of G3 Character Set | `ESC O` | `1B 4F` | `8F` |
| DCS | Device Control String | `ESC P` | `1B 50` | `90` |
| SPA | Start of Guarded Area | `ESC V` | `1B 56` | `96` |
| EPA | End of Guarded Area | `ESC W` | `1B 57` | `97` |
| SOS | Start of String | `ESC X` | `1B 58` | `98` |
| DECID | Return Terminal ID | `ESC Z` | `1B 5A` | `9A` |
| CSI | Control Sequence Introducer | `ESC [` | `1B 5B` | `9B` |
| ST | String Terminator | `ESC \` | `1B 5C` | `9C` |
| OSC | Operating System Command | `ESC ]` | `1B 5D` | `9D` |
| PM | Privacy Message | `ESC ^` | `1B 5E` | `9E` |
| APC | Application Program Command | `ESC _` | `1B 5F` | `9F` |

## Save Cursor

一時的にカーソル（や一部描画情報など）を保存、復元するための機能。
現代ではALT-Screen/Primary-Screenを跨いでカーソルを復元するためにも使用される。

### DECSC

Reference: https://vt100.net/docs/vt510-rm/DECSC.html
ESC: `ESC 7`

以下の状態をメモリ上に保存する。命令には各種パラメータは割り当てられないため保存する状態は端末側で管理する必要がある。

| 状態 | 説明 |
| --- | --- |
| カーソル位置 | 現在の行と列を保存する。 |
| SGR文字属性 | SGRで設定された前景色、背景色、太字、下線などの文字属性を保存する。 |
| G0–G3およびGL/GR | G0–G3に指示された文字集合と、GLおよびGRに呼び出されている文字集合を保存する。 |
| Wrap flag | 自動折り返しを行うかどうかを示すDECAWMの状態を保存する。 |
| Origin mode | カーソル位置の基準を画面全体またはスクロール領域とするDECOMの状態を保存する。 |
| Selective erase attribute | 以後に書き込む文字を選択消去の対象とするか、保護対象とするかを示す属性を保存する。 |
| SS2／SS3 | 次の1文字に対してG2またはG3を一時的に呼び出す、未適用のsingle shift状態を保存する。 |

Orzmaでは`SavedCursor`という構造体を`Screen`内に保持している。保存時にはこの構造体に現在の状態を書き込む。
保存する情報の一覧は以下。

