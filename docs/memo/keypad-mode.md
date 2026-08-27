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
CSI ? 6 6 I # Reset: keypad characters.
```

## テンキーシーケンス

- [definition](../../docs/references/vt510.pdf#page=369)

Table 8-3: PC Layout Numeric Keypad Sequences - VT Style

| PC Key | DEC Key | Numeric Mode | Application Mode |
| --- | --- | --- | --- |
| `Num Lock` | `PF1` | `SS3 P` | `SS3 P` |
| `/` | `PF2` | `SS3 Q` | `SS3 Q` |
| `*` | `PF3` | `SS3 R` | `SS3 R` |
| `-` | `PF4` | `SS3 S` | `SS3 S` |
| `Caps Lock/+` | `-` |  | `SS3 m` |
| `+` | `,` | `"+"` | `SS3 l` |
| `.` | `.` | `"."` | `SS3 n` |
| `Enter` | `Enter` | `CR` | `SS3 M` |
| `0` | `0` | `"0"` | `SS3 p` |
| `1` | `1` | `"1"` | `SS3 q` |
| `2` | `2` | `"2"` | `SS3 r` |
| `3` | `3` | `"3"` | `SS3 s` |
| `4` | `4` | `"4"` | `SS3 t` |
| `5` | `5` | `"5"` | `SS3 u` |
| `6` | `6` | `"6"` | `SS3 v` |
| `7` | `7` | `"7"` | `SS3 w` |
| `8` | `8` | `"8"` | `SS3 x` |
| `9` | `9` | `"9"` | `SS3 y` |
