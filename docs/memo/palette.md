# Palette

## 構造

VTは１つパレットを保持する。

セルが持つのは色番号だけで、描画時にパレットの`n`番目を引く。
そのためパレットのスロットを書き換えると、すでに画面に出ている文字の色も変わる。
`38;2;r;g;b`で直接指定したセル（`Color::Rgb`）はパレットを経由しないので変わらない。

色にはいくつかの種類（ANSI Colors / Special Colors / Dynamic Colors）があり、種類によって**どの OSC でどのスロットを指すか**が異なる。
色の値の書式（`color_spec`）はどの種類でも共通で、Xlib の Color String を使う。

`orzma`では以下のような構造にする予定。
```rust
pub struct Palette {
    pub indexed: Box<[Rgb; 256]>,
    pub foreground: Rgb,
    pub background: Rgb,
}
```

## 色の精度

パレットは 1 チャンネル 8 ビット（`Rgb { r: u8, g: u8, b: u8 }`）のまま持つ。

Xlib は色を 1 チャンネル 16 ビット（0〜65535）で扱う（[xlib.pdf](../references/xlib.pdf#page=85) p.72）ため、`color_spec`を解釈した値は 8 ビットへ丸める必要がある。

- 丸め方: 16 ビットにそろえた値の上位 8 ビットを取る（`Rgb::from_color_spec`）。`rgb:`はスケール規則の逆（257 で割る）と結果が一致するが、`#`は上位ビット詰めなので一致しない（`#f00`の赤は 0xf0、257 で割ると 0xef）。
- `?`の応答: 8 ビットの値を 2 回並べた 4 桁で返す（`0x12` → `rgb:1212/…`）。これは`rgb:hh`を Xlib の規則で 16 ビットに広げた値と一致する。Alacritty も同じ形式。

16 ビットで持たない理由:

- 画面に差が出ない。レンダラは色を`u32`に詰めて GPU に渡し（`PackedPalette::build`）、ディスプレイも一般に 8 ビット。
- `Rgb`はセル（`Cell.fg / bg`の`Color::Rgb`）と共有しており、広げると全セルとスクロールバックのメモリが増える。
- SGR `38;2;r;g;b`の値はもともと 0〜255（xterm-ctlseqs.pdf p.25）。
- 失うのは「`?`で設定値がそのまま返る」ことだけ。応答を書き戻せば同じ色に戻るので、保存・復元の用途は壊れない。

## Dynamic Color

番号ではなく役割（テキスト前景・背景、カーソル、選択範囲など）で識別される 10 色。
ANSI パレット（OSC 4）とは別の色だが、SGR 39 / 49（既定色）で描かれたセルは
テキスト前景・背景の dynamic color を参照する。

すべての種別(Resource)は`xterm-ctlseqs.pdf`の p.39〜40 に記載されている。
`orzma`が持つのは`foreground`（OSC 10）と`background`（OSC 11）の２つだけで、残りの８色に対応するものは無い。
`cursorColor`（OSC 12）は実装予定（`device.rs`の`ColorTable`に TODO がある）。

## ANSI Colors

色の集合を配列で管理する。
色の更新は`OSC 4`、リセットは`OSC 104`で行う。

### OSC 4

形式
```
OSC 4 ; <color_index> ; <color_spec> [ ; <color_index> ; <color_spec> … ] BEL （または ST）
OSC 4 ; <color_index> ; ? BEL （または ST）
```

- `color_index ; color_spec`のペアは１本の中に何組でも並べられる。
- `color_spec`の代わりに`?`を置くと問い合わせになり、設定と同じ形式の応答が返る。応答の終端は問い合わせと同じもの（BEL か ST）を使う。

`color_index`の 0〜255 は`ANSI Colors`の対象範囲（0〜7 が ANSI colors、8〜15 がその bright 版、16〜255 が 256 色表の残り）。
256〜260 は`Special Colors`（256 + Pc、Pc は 0〜4）で、261 以上は定義されていない。
256 を足すのは`orzma`が 256 色（`colors#256`）を名乗っているからで、88 色の xterm なら 88 + Pc になる。

`color_index`は`u8`に切り詰めずに広い整数型で読み、範囲を確かめてから使う。`256 as u8`は 0 になり、Special Colors の指定がスロット 0 を上書きしてしまう。

`color_spec`は`XParseColor`に渡す文字列として定義されており（xterm-ctlseqs.pdf p.38）、`XParseColor`が受け取るのは Color String である。
Color String は[xlib.pdf](../references/xlib.pdf#page=89)の p.76 に記載されている。大文字小文字は区別しない。

実際によく来る形:

- `rgb:ff/80/00` — terminfo の`initc`が送る形。`rgb:`は桁数でスケールする（`rgb:f` = `rgb:ffff`）。
- `#3a7` — 旧形式。上位ビットとして扱う（`#3a7` = `#3000a0007000`で、`rgb:3/a/7` = `rgb:3333/aaaa/7777`とは別の色）。
- `red` — 色名。名前の一覧は X サーバーの色名データベース次第で、Xlib は定めていない。

その他の Color String の例（xlib.pdf p.76）:

- "CIEXYZ:0.3227/0.28133/0.2493"
- "RGBi:1.0/0.0/0.0"
- "rgb:00/ff/00"
- "CIELuv:50.0/0.0/0.0"

## Special Colors

bold / underline / blink / reverse / italic の文字を描くときに使う５色（colorBD / UL / BL / RV / IT）。
番号では選べず、属性を持つ文字に、その色モードが有効（OSC 6 / 106）なときだけ使われる。

- 設定: `OSC 5 ; Pc ; spec`（Pc は 0〜4）。`OSC 4 ; 256 + Pc ; spec`でも同じ。
- リセット: `OSC 105`。`OSC 104 ; 256`で Special Colors がリセットされるかは資料に書かれていない。

## References

- [xterm-ctlseqs.pdf](../references/xterm-ctlseqs.pdf)
- [xlib.pdf](../references/xlib.pdf)
