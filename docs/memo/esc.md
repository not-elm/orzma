# ESC

ESCはC0の一種であり、制御関数を拡張するために使用される。
`ESC(0x1B)`から始まるその命令を表すシーケンスはエスケープシーケンスと呼称され、命令によって細部の構造が異なる。

また、C1は8ビットで制御関数を表すが、ESCを使って７ビット環境向けに同じ関数を提供することができる。
UTF8と競合が発生しないなどの理由から現代でもデフォルトでESCを利用されることが多い。[ctrlseqs](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)によるとS8C1Tが1の時8ビットで

## 7bit形式(ESC)と8bit形式(C1)

ESCの中にはC1の制御関数を7ビット環境で再現するために各C1に対応した命令が用意されている。

| 制御関数 | 説明                                    | 7bit形式 | 7bitバイト列 | 8bit形式(C1) |
| -------- | --------------------------------------- | -------- | ------------ | ------------ |
| IND      | Index                                   | `ESC D`  | `1B 44`      | `84`         |
| NEL      | Next Line                               | `ESC E`  | `1B 45`      | `85`         |
| HTS      | Tab Set                                 | `ESC H`  | `1B 48`      | `88`         |
| RI       | Reverse Index                           | `ESC M`  | `1B 4D`      | `8D`         |
| SS2      | Single Shift Select of G2 Character Set | `ESC N`  | `1B 4E`      | `8E`         |
| SS3      | Single Shift Select of G3 Character Set | `ESC O`  | `1B 4F`      | `8F`         |
| DCS      | Device Control String                   | `ESC P`  | `1B 50`      | `90`         |
| SPA      | Start of Guarded Area                   | `ESC V`  | `1B 56`      | `96`         |
| EPA      | End of Guarded Area                     | `ESC W`  | `1B 57`      | `97`         |
| SOS      | Start of String                         | `ESC X`  | `1B 58`      | `98`         |
| DECID    | Return Terminal ID                      | `ESC Z`  | `1B 5A`      | `9A`         |
| CSI      | Control Sequence Introducer             | `ESC [`  | `1B 5B`      | `9B`         |
| ST       | String Terminator                       | `ESC \`  | `1B 5C`      | `9C`         |
| OSC      | Operating System Command                | `ESC ]`  | `1B 5D`      | `9D`         |
| PM       | Privacy Message                         | `ESC ^`  | `1B 5E`      | `9E`         |
| APC      | Application Program Command             | `ESC _`  | `1B 5F`      | `9F`         |

## 主要な関数一覧

| ESC                                                                         | Page NO |
| --------------------------------------------------------------------------- | ------- |
| [RIS - Reset to Initial State](../../docs/references/vt510.pdf#page=331)    | 331     |
| [DECKPAM—Keypad Application Mode](../../docs/references/vt510.pdf#page=178) | 178     |

### CSI

CSIは、主にカーソル移動、画面の編集、文字の装飾、端末の設定や状態問い合わせを行う命令の開始記号。`ESC [ `から始まる。

| CSI                                                                           | Page NO |
| ----------------------------------------------------------------------------- | ------- |
| [ICH — Insert Character](../../docs/references/vt510.pdf#page=316)            | 316     |
| [DCH — Delete Character](../../docs/references/vt510.pdf#page=121)            | 121     |
| [ECH — Erase Character](../../docs/references/vt510.pdf#page=309)             | 309     |
| [DECTCEM — Text Cursor Enable Mode](../../docs/references/vt510.pdf#page=282) | 282     |
| [IRM — Insert/Replace Mode](../../docs/references/vt510.pdf#page=319)         | 319     |
