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

- [xterm: VT220-Style Function Keys](../../docs/references/xterm-ctlseqs.pdf#page=49) — **これがキーパッドの仕様**
- [xterm: PC-Style Function Keys](../../docs/references/xterm-ctlseqs.pdf#page=46) — キーパッドの仕様ではない（後述）
- [VT510 Table 8-3: PC Layout Numeric Keypad Sequences - VT Style](../../docs/references/vt510.pdf#page=369)
- [VT510 Table 8-4: PC Layout Numeric Keypad Sequences - PC Style, Numeric Mode](../../docs/references/vt510.pdf#page=370)

VT510 Table 8-3は`PC Key`と`DEC Key`の2列で引く表だが、xtermの表は物理キー1本で引く。統合にあたって行キーを物理キーへ寄せ、`DEC Key`は下の差分表にのみ残した。実装する`KeypadKey`のバリアントもこの行キーに対応する。

| PC キー | Numeric | Application |
| --- | --- | --- |
| `/` | `/` | `SS3 o` |
| `*` | `*` | `SS3 j` |
| `-` | `-` | `SS3 m` |
| `+` | `+` | `SS3 k` |
| `,` | `,` | `SS3 l` |
| `=` | `=` | `SS3 X` |
| `Enter` | `CR` | `SS3 M` |
| `.` | `.` | `SS3 n` |
| `0` | `0` | `SS3 p` |
| `1` | `1` | `SS3 q` |
| `2` | `2` | `SS3 r` |
| `3` | `3` | `SS3 s` |
| `4` | `4` | `SS3 t` |
| `5` | `5` | `SS3 u` |
| `6` | `6` | `SS3 v` |
| `7` | `7` | `SS3 w` |
| `8` | `8` | `SS3 x` |
| `9` | `9` | `SS3 y` |
| `Num Lock` | 送らない | 送らない |
| `Space` † | `SP` | `SS3 SP` |
| `Tab` † | `TAB` | `SS3 I` |
| `PF1`–`PF4` † | `SS3 P`–`SS3 S` | `SS3 P`–`SS3 S` |

† PCテンキーには物理キーが存在しない。DEC/Sunキーボードからのみ到達する。xtermは「Not all keys are present on the Sun/PC keypad (e.g., PF1, Tab), but are supported by the program.」として語彙には残している。

`Num Lock`はPTYへ送らない。xtermは「Use the NumLock key to override the application mode.」と書いており、Applicationモードを一時的に打ち消すローカルキーとして扱う。X11では標準でモディファイア扱いされず、`numLock`リソースか`DECSET 1035`で有効化される。

`SS3`に続くバイトは、多くのキーで「Numericモードで送る文字 + 0x40」になっている。`,`(0x2C)→`l`(0x6C)、`-`(0x2D)→`m`、`.`(0x2E)→`n`、`/`(0x2F)→`o`、数字(0x30–0x39)→`p`–`y`、`Tab`(0x09)→`I`(0x49)、`Enter`はCR(0x0D)→`M`(0x4D)。ただし`Space`(`SS3 SP`)、`=`(`SS3 X`)、`PF1`–`PF4`はこの規則から外れるので、計算ではなく表として持つこと。

### xterm の "PC-Style Function Keys" 表について

p.46-47 にもキーパッドの表があり、そちらは数字キーの Application 列に `CSI 2~`(Ins) や `CSI B`(↓)、`SS3 H`(Home) を並べている。**あれはキーパッドモードの表ではない**ので、上の表と混ぜてはいけない。

xterm には DECKPAM で分岐するキーパッド経路が1本しかなく（`input.c:1390`）、そこは無条件に `kypd_apl` を引いて `SS3 p`–`SS3 y` を返す:

```c
} else if (IsKeypadKey(kd.keysym)) {
    if (keypad_mode) {
        reply.a_type = ANSI_SS3;
        reply.a_final = (Char) (kypd_apl[kd.keysym - XK_KP_Space]);
```

PC-Style 表の `CSI 2~` / `CSI B` / `SS3 F` は、NumLock が OFF のときに来る**別の keysym** を書き換える経路の出力である（`input.c:1177`）:

```c
if (kd.keysym >= XK_KP_Home && kd.keysym <= XK_KP_Begin) {
    kd.keysym += (KeySym) (XK_Home - XK_KP_Home);
}
```

XKB は物理キー1つに2つの keysym を持たせ、NumLock で段を選ぶ:

```
key <KP7> { [ KP_Home, KP_7 ] };
type "KEYPAD" { modifiers = NumLock; map[None] = Level1; map[NumLock] = Level2; };
```

つまり PC-Style 表は、**物理キー1つが持つ2つの役割を1行に併記した刻印の説明**であって、モードの表ではない。表の Application 列が `SS3 H`/`SS3 F`（DECCKM application 形）と `CSI A`/`CSI B`/`CSI E`（DECCKM normal 形）を混在させていて、5キーすべてが同じ経路・同じフラグを通る以上**単一のモード状態では produce できない**ことが、その証拠になっている。

この読み違いは実装系にも存在する。WezTerm（`termwiz/src/input.rs`）はこの表を数字→編集キー変換として実装し、`Numpad9 => "\x1b[6"`（PageUp なら `\x1b[5`）という転記バグまで抱えている。

実装系の分布（一次ソース確認済み）:

| 実装 | Application モードの数字キー |
| --- | --- |
| xterm, foot, ghostty, urxvt | `SS3 p`–`SS3 y` |
| Terminal.app | `SS3 p`–`SS3 y`。ただし表は厳密に VT100 で `* + / =` と PF1–PF4 を持たない |
| iTerm2 | `SS3 p`–`SS3 y`。ただし `TERM` に "xterm" を含むときのみ DECKPAM を受理する |
| Windows Terminal | `SS3 p`–`SS3 y` を実装しているが、機能フラグ `Feature_KeypadModeEnabled` が `AlwaysDisabled`（`Dev` ビルドのみ有効）なので出荷版は数字のまま |
| VTE, Konsole, kitty, Alacritty | DECKPAM の数字符号化を実装していない |
| WezTerm | PC-Style 編集シーケンス（唯一の例）。ただし macOS では到達しない |

terminfo も一致する。`xterm` / `xterm-256color` は `ka1=\EOw kb2=\EOu kc1=\EOq kpZRO=\EOp kpDOT=\EOn` に解決し、macOS 同梱の `nsterm` も `ka1=\EOq kb2=\EOr kc1=\EOp kc3=\EOn kent=\EOM smkx=\E[?1h\E=` と SS3 形で記述している。`vim` も `src/term.c` に `{K_K0, "\033O*p"}` … `{K_K9, "\033O*y"}` をハードコードしている。

**macOS では反例が存在しない。** DECKPAM を実装している2つ（Terminal.app と iTerm2）はどちらも `SS3 p`–`SS3 y` を送り、唯一の反例である WezTerm はその経路に macOS から到達できない。

Home / End / Insert / Delete / PageUp / PageDown / 矢印 / Begin は、キーパッドの別モードではなく**別のキー**として扱う。orzma では `TerminalKey` 側の識別子として持ち、DECKPAM ではなく DECCKM で分岐させるのが正しい配置になる。

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

Table 8-4は「PC Style, Numeric Mode」の表で、`Num Lock`がOffなら`7`が`CSI H`を送る、という編集キー挙動を定義している。

これはxtermと矛盾しない。どちらも「`Num Lock`がOffのとき、その物理キーは編集キーとして振る舞う」と言っており、xterm側ではそれがキーマップ層の別keysymとして現れる。VT510はキーマップ層を持たない単体端末なので、同じことを自前の表として書いている。

いずれにせよ`KeypadKey::encode`が扱う範囲の外である。このメソッドは解決済みの`KeypadMode`を受け取るだけで、どちらの役割で押されたかは呼び出し側が決める。

macOSにはそもそも`Num Lock`が無く、XKBもMac用に`type "KEYPAD" { modifiers = None; map[None] = Level2; }`（数字を無条件に選ぶ）を持つので、主対象プラットフォームでは編集キーとしての役割が存在しない。
