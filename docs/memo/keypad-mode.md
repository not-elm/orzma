# Application Keypad Mode

[Application Keypad Mode](../../docs/references/vt510.pdf#page=44)
> This feature selects whether the numeric keypad sends ASCII numerals or application function
sequences. It corresponds to the DECKPNM, DECKPAM, and DECNKM control functions described in
Chapter 5, ANSI Control Functions.This field is not stored in NVR. This field is reset to the power-up setting when a soft reset occurs (Reset
Session or receipt of DECSTR).This field is not a user-preference feature. It cannot be locked. Changes to this field take effect
immediately so you can use the keypad to enter an Answerback message.

以下のようなEnumで表現する予定

```rust
pub enum KeypadMode{
    Numeric,
    Application
}
```

以下の説明におけるPFというキーはDEC端末におけるファンクションキー群を指すらしい。現代のキーボードには（おそらく）存在しないが代わりに`NumLock`, `/`, `*`, `-`を使うらしいが自分のキーボードにテンキーがないためよくわからない。
> The enhanced PC layout numeric keypad has three differences from the VT layout:1. The four keys at the top of the keyboard are labeled  Num Lock ,  / ,  * , and  -  instead of  PF1
through  PF4 .2. The  Num Lock  key toggles the keypad keys, sending either numerals or

## 制御関数一覧

### DECKPNM—Keypad Numeric Mode

- [definition](../../docs/references/vt510.pdf#page=180)

`KeypadMode::Numeric`にモードを遷移     
テンキーが数字を文字として送信するようにする。

### DECKPAM—Keypad Application Mode

- [definition](../../docs/references/vt510.pdf#page=178)

`KeypadMode::Aplication`にモードを遷移      
テンキーからホストへ送信する際に、アプリケーションシーケンスを利用するようになる。

### DECNKM—Numeric Keypad Mod

- [definition](../../docs/references/vt510.pdf#page=191)

> This control function works like the DECKPAM and DECKPNM functions. DECNKM is provided mainly for use with the request and report mode (DECRQM/DECRPM) control functions.

```
CSI ? 6 6 h # Set: application sequences.
CSI ? 6 6 l # Reset: keypad characters.
```

final byteは小文字の`l`。PDFの字形が大文字`I`と紛らわしいが、定義ページがfinal byteの下に併記しているcolumn/rowコードが`6/8`(=0x68=`h`)と`6/12`(=0x6C=`l`)なので一意に決まる。大文字`I`なら`4/9`になる。

## テンキーシーケンス

- [VT510 Table 8-3: PC Layout Numeric Keypad Sequences - VT Style](../../docs/references/vt510.pdf#page=369)
- [VT510 Table 8-4: PC Layout Numeric Keypad Sequences - PC Style, Numeric Mode](../../docs/references/vt510.pdf#page=370)
- [xterm: PC-Style Function Keys](../../docs/references/xterm-ctlseqs.pdf#page=46)
- [xterm: VT220-Style Function Keys](../../docs/references/xterm-ctlseqs.pdf#page=49)

VT510 Table 8-3は`PC Key`と`DEC Key`の2列で引く表だが、xtermの表は物理キー1本で引く。統合にあたって行キーを物理キーへ寄せ、`DEC Key`は下の差分表にのみ残した。実装する`KeypadKey`のバリアントもこの行キーに対応する。

Numeric列はxtermの2スタイルで完全に同一なので1列に畳んである。Application列は`.`と数字`0`–`9`の11行だけがスタイルで分かれる。

| PC キー | Numeric | Application (PC-Style) | Application (VT220-Style) |
| --- | --- | --- | --- |
| `/` | `/` | `SS3 o` | 同左 |
| `*` | `*` | `SS3 j` | 同左 |
| `-` | `-` | `SS3 m` | 同左 |
| `+` | `+` | `SS3 k` | 同左 |
| `,` | `,` | `SS3 l` | 同左 |
| `=` | `=` | `SS3 X` | 同左 |
| `Enter` | `CR` | `SS3 M` | 同左 |
| `.` | `.` | `CSI 3 ~` | `SS3 n` |
| `0` | `0` | `CSI 2 ~` | `SS3 p` |
| `1` | `1` | `SS3 F` | `SS3 q` |
| `2` | `2` | `CSI B` | `SS3 r` |
| `3` | `3` | `CSI 6 ~` | `SS3 s` |
| `4` | `4` | `CSI D` | `SS3 t` |
| `5` | `5` | `CSI E` | `SS3 u` |
| `6` | `6` | `CSI C` | `SS3 v` |
| `7` | `7` | `SS3 H` | `SS3 w` |
| `8` | `8` | `CSI A` | `SS3 x` |
| `9` | `9` | `CSI 5 ~` | `SS3 y` |
| `Num Lock` | 送らない | 送らない | 同左 |
| `Space` † | `SP` | `SS3 SP` | 同左 |
| `Tab` † | `TAB` | `SS3 I` | 同左 |
| `PF1`–`PF4` † | `SS3 P`–`SS3 S` | `SS3 P`–`SS3 S` | 同左 |

† PCテンキーには物理キーが存在しない。DEC/Sunキーボードからのみ到達する。xtermは「Not all keys are present on the Sun/PC keypad (e.g., PF1, Tab), but are supported by the program.」として語彙には残している。

`Num Lock`はPTYへ送らない。xtermは「Use the NumLock key to override the application mode.」と書いており、Applicationモードを一時的に打ち消すローカルキーとして扱う。X11では標準でモディファイア扱いされず、`numLock`リソースか`DECSET 1035`で有効化される。

`SS3`に続くバイトは、多くのキーで「Numericモードで送る文字 + 0x40」になっている。`,`(0x2C)→`l`(0x6C)、`-`(0x2D)→`m`、`.`(0x2E)→`n`、`/`(0x2F)→`o`、数字(0x30–0x39)→`p`–`y`、`Tab`(0x09)→`I`(0x49)、`Enter`はCR(0x0D)→`M`(0x4D)。ただし`Space`(`SS3 SP`)、`=`(`SS3 X`)、`PF1`–`PF4`はこの規則から外れるので、計算ではなく表として持つこと。

### VT510 Table 8-3 との差分

上の表とTable 8-3が食い違うのは以下の6行のみ。他の行(`.` / `Enter` / 数字)はTable 8-3のApplication列が上の表のVT220-Style列と完全に一致する。つまり **Table 8-3 = VT220-Style + 上段4キーの位置読み替え** という関係にある。

| PC キー | DEC Key | VT510 Table 8-3 | 上の表 |
| --- | --- | --- | --- |
| `Num Lock` | `PF1` | 両モード `SS3 P` | 送らない |
| `/` | `PF2` | 両モード `SS3 Q` | `/` / `SS3 o` |
| `*` | `PF3` | 両モード `SS3 R` | `*` / `SS3 j` |
| `-` | `PF4` | 両モード `SS3 S` | `-` / `SS3 m` |
| `+` | `,` | `"+"` / `SS3 l` | `+` / `SS3 k` |
| `Caps Lock/+` | `-` | （空欄） / `SS3 m` | 該当キー無し |

DECキーパッドと PCテンキーは同じ位置に違うキーが並ぶ。Table 8-3はタイトルが示す通り「PC配列のキーパッドをVTキーパッドとして振る舞わせる」表なので、対応がキーの意味ではなく**位置**で取られている。上段4キーはPF1–PF4の位置、`+`はDECの`-`/`,`の位置にあたる。

xtermはこの位置読み替えを採用していない。`/`と`PF2`を別々の行として並べていることがその証拠で、PC配列で`-`を押してもPF4は飛ばない。`Caps Lock/+`は現代のテンキーに対応キーがなく到達しない。

なお、この差分表の`+`→`,`の行だけはxtermにも実装がある。ただし`OPT_SUNPC_KBD`ビルドかつVT220キーボード種別を明示選択したときのみで、キー名ではなくkeysymの書き換えとして処理される。

### VT510 Table 8-4 との差分

Table 8-4は「PC Style, Numeric Mode」の表で、`Num Lock`がOffなら**Numericモードで**`7`が`CSI H`を送る、という編集キー挙動を定義している。

xtermは同じ編集シーケンスを**Applicationモード側**に置き、`Num Lock`をそれを打ち消すスイッチとして使う。到達する状態は近いが、編集シーケンスをどちらのモードに紐づけるかが逆なので、上の表に混ぜてはいけない。
