# OS/2 - OS/2 and Windows Metrics Table

- [definition](https://learn.microsoft.com/en-us/typography/opentype/spec/os2) (OpenType 1.9.1)
- [recommendations](https://learn.microsoft.com/en-us/typography/opentype/spec/recom#baseline-to-baseline-distances)

OpenType フォントのテーブルの1つ。行送りに使う縦メトリクス、取り消し線の位置と太さ、ウェイトや幅のクラス、埋め込みライセンス、対応文字範囲など、フォント全体にかかる値をまとめて持つ。

> The OS/2 table consists of a set of metrics and other data that are required in OpenType fonts.

orzma の取り消し線（`Baseline::strike_depth`）は、このテーブルの `yStrikeoutPosition` から位置を決めている。

## 背景

OS/2 プラットフォームとの互換に由来するフィールドを持つテーブルである。仕様の `fsSelection` の説明に「OS/2 プラットフォームとの互換のために作られた」とあり、`sFamilyClass` の分類値も IBM が定義したものである。テーブルの正式名称が「OS/2 and Windows metrics table」であるとおり、Windows 向けのメトリクス（`usWinAscent` など）も含む。Microsoft の version 0 の定義には、すでに `sTypo*` と `usWin*` の両方がある。

初期の Mac の TrueType は行送りを `hhea` から取り、Windows GDI は OS/2 の `usWin*` から取っていた。これらの値はプラットフォームごとの互換性に制約されるため、`sTypo*` は「プラットフォームに依存しない行送り」を指定するための値である。version 4 で追加された `fsSelection` bit 7（`USE_TYPO_METRICS`）は、typo の行送りを使うようアプリケーションに推奨するフラグである。既存文書のリフローを避けるため、古いフォントへの追加には注意が求められる。

## バージョン

0 から 5 まで定義されている。仕様は version 4 以上を強く推奨する。

| version | 定義元 | テーブル長 | 追加フィールド |
| ------- | ------ | ---------- | -------------- |
| 0 | TrueType 1.5 | 78 | 基本フィールド（`usWinDescent` まで） |
| 1 | TrueType 1.66 | 86 | `ulCodePageRange1-2` |
| 2 | OpenType 1.1 | 96 | `sxHeight`, `sCapHeight`, `usDefaultChar`, `usBreakChar`, `usMaxContext` |
| 3 | OpenType 1.4 | 96 | なし（Unicode 3.2 に合わせて既存フィールドの定義を改訂） |
| 4 | OpenType 1.5 | 96 | なし（`fsSelection` bit 7-9 を定義、`ulUnicodeRange` を Unicode 5.1 まで拡張） |
| 5 | OpenType 1.7 | 100 | `usLowerOpticalPointSize`, `usUpperOpticalPointSize` |

version 5 の導入元は [OpenType 1.7 の変更履歴](https://learn.microsoft.com/en-us/typography/opentype/spec/changes#version-17)で確認できる。2015年3月のリリースに、OS/2 を version 5.0 に更新し、光学サイズの2フィールドを追加したことが明記されている。

Apple の TrueType Reference Manual の version 0 は `usLastCharIndex`（オフセット 66）で終わっている。そのため長さ 68 の短い version 0 テーブルを持つ古いフォントがあり、仕様は「version 0 では最後の5フィールドを読む前にテーブル長を確認せよ」としている。

## フィールド一覧

オフセットはテーブル先頭からのバイト位置。単位の「FU」はフォントデザイン単位（`head.unitsPerEm` で1em）。

| offset | 型 | フィールド | 単位 | 内容 |
| ------ | -- | ---------- | ---- | ---- |
| 0 | uint16 | `version` | | 0〜5 |
| 2 | FWORD | `xAvgCharWidth` | FU | 幅ゼロでない全グリフの平均送り幅。レイアウトに使うべきではない |
| 4 | uint16 | `usWeightClass` | | 1〜1000。400 = Regular、700 = Bold。`wght` 軸と同じ尺度 |
| 6 | uint16 | `usWidthClass` | | 1〜9。5 = Normal |
| 8 | uint16 | `fsType` | | 埋め込みライセンスのフラグ |
| 10〜24 | FWORD ×8 | `ySubscript*` / `ySuperscript*` | FU | 下付き・上付きの推奨サイズとオフセット |
| 26 | FWORD | `yStrikeoutSize` | FU | 取り消し線の太さ |
| 28 | FWORD | `yStrikeoutPosition` | FU | 取り消し線の**上端**のベースラインからの高さ。上が正 |
| 30 | int16 | `sFamilyClass` | | IBM のファミリー分類（上位バイト = class、下位 = subclass） |
| 32 | uint8[10] | `panose` | | PANOSE 分類 |
| 42 | uint32 ×4 | `ulUnicodeRange1-4` | | 対応 Unicode ブロックのビット集合（128 bit） |
| 58 | Tag | `achVendID` | | ベンダー ID（4文字） |
| 62 | uint16 | `fsSelection` | | スタイルフラグ（後述） |
| 64 | uint16 | `usFirstCharIndex` | | Microsoft cmap（platform 3、encoding 0/1）の最小 Unicode インデックス。最小値が BMP 外なら 0xFFFF |
| 66 | uint16 | `usLastCharIndex` | | 同 cmap の最大 Unicode インデックス。補助平面の文字を含むフォントでは 0xFFFF |
| 68 | FWORD | `sTypoAscender` | FU | 組版用アセンダー |
| 70 | FWORD | `sTypoDescender` | FU | 組版用ディセンダー。**通常は負** |
| 72 | FWORD | `sTypoLineGap` | FU | 組版用の行間 |
| 74 | UFWORD | `usWinAscent` | FU | Windows のクリップ領域の上端 |
| 76 | UFWORD | `usWinDescent` | FU | Windows のクリップ領域の下端。**下向きが正** |
| 78 | uint32 ×2 | `ulCodePageRange1-2` | | 対応コードページのビット集合（v1+） |
| 86 | FWORD | `sxHeight` | FU | x-height（v2+） |
| 88 | FWORD | `sCapHeight` | FU | 大文字の高さ（v2+） |
| 90 | uint16 | `usDefaultChar` | | 欠字のときの代替文字（v2+、使用非推奨） |
| 92 | uint16 | `usBreakChar` | | 語区切り文字。通常 U+0020（v2+、使用非推奨） |
| 94 | uint16 | `usMaxContext` | | GSUB/GPOS の最大文脈長（v2+） |
| 96 | uint16 | `usLowerOpticalPointSize` | TWIP | 光学サイズ範囲の下限（v5、STAT で置き換え） |
| 98 | uint16 | `usUpperOpticalPointSize` | TWIP | 光学サイズ範囲の上限（v5、STAT で置き換え） |

符号の向きがフィールドごとに違う点に注意する。`sTypoDescender` と `hhea.descender` は下向きが負、`usWinDescent` は下向きが正（符号なし）である。

`ySubscriptYOffset` も下向きの距離を正で表す。一方、`ySuperscriptYOffset` は上向きの距離を正で表す。型が同じ FWORD でも、オフセットを一律に y 座標として扱うことはできない。

## 縦メトリクス

### 3組の値

行送りに関わる値は、フォント内に3組ある。

| 組 | フィールド | 本来の用途 |
| -- | ---------- | ---------- |
| hhea | `ascender`, `descender`, `lineGap` | Mac の行送り |
| OS/2 typo | `sTypoAscender`, `sTypoDescender`, `sTypoLineGap` | プラットフォーム非依存の行送り |
| OS/2 win | `usWinAscent`, `usWinDescent` | Windows GDI のクリップ領域（行間は持たない） |

仕様上の行送り（baseline-to-baseline）は typo で次のとおり計算する。

```
sTypoAscender - sTypoDescender + sTypoLineGap
```

`sTypoAscender` は「テキストフレームの上端から最初のベースラインまで」の推奨オフセットとしても使える。`sTypoAscender - sTypoDescender` を1emにするのが一般的だが必須ではない。`sTypoLineGap` は行送りが1emの120%前後になるよう設定されることが多い（例: Minion Pro は 1000 UPM で gap 200）。

### `USE_TYPO_METRICS`（`fsSelection` bit 7）

> If set, it is strongly recommended that applications use OS/2.sTypoAscender - OS/2.sTypoDescender + OS/2.sTypoLineGap as the default line spacing for this font.

version 4 で定義された。version 0〜3 では bit 7 は予約で、アプリケーションは無視すべきとされる。新しいフォントは bit 7 を立てるよう推奨されている。一方、古いフォントを更新して bit 7 を立てると既存文書がリフローするので、`usWin*` での行送りが著しく劣る場合に限るべきとされている。

### `usWin*` とクリップ

Windows GDI は TrueType ラスタライザのビットマップ面の大きさを `usWinAscent` / `usWinDescent` で決め、その外にはみ出したアウトラインを**切り取る**。クリップさせたくなければ `usWinAscent >= yMax`、`usWinDescent >= -yMin` にする。これは GPOS / kern 適用前のグリフ位置に対する説明である。VDMX テーブルに現在のデバイスの縦横比とラスタライズサイズに対応するデータがあれば、そちらが優先される。

`usWin*` を行送りに使うことは「強く非推奨」とされているが、一部のレガシーアプリはこの値を使う。bit 7 に応じて typo と win を切り替えるアプリもある。仕様の recom が示す初期の TrueType 向け GDI `TEXTMETRIC` の対応は次のとおりである。

| GDI | OpenType |
| --- | -------- |
| `tmAscent` | `usWinAscent` |
| `tmDescent` | `usWinDescent` |
| `tmInternalLeading` | `usWinAscent + usWinDescent - unitsPerEm` |
| `tmExternalLeading` | `MAX(0, (hhea.ascender - hhea.descender + hhea.lineGap) - (usWinAscent + usWinDescent))` |

### 実装ごとの選び方

OS 名だけでは選び方は決まらない。以下は仕様に記された初期の実装と、今回確認したソースの描画経路ごとの選択である。公開実装は2026-10-13に参照した各リポジトリの main / master のコードであり、特定の配布版での実測ではない。

| 実装 | 選び方 | 典拠 |
| ---- | ------ | ---- |
| 初期の Macintosh TrueType | `hhea` | OpenType recom |
| 初期の Windows GDI（TrueType） | `usWin*` | OpenType recom |
| FreeType（`face->ascender` など） | bit 7 が立っていれば typo → それ以外は `hhea` → `hhea` の ascender と descender がともに 0 なら typo → typo も 0 なら `usWin*` | `src/sfnt/sfobjs.c` |
| Ghostty（FreeType / CoreText 経路） | `hhea` と OS/2 を直接読み、FreeType と同じ順・ゼロ判定で選ぶ。`hhea` が取れない場合は、それぞれ FreeType のサイズメトリクス / CoreText の API 値 | `src/font/face/freetype.zig`, `coretext.zig` |
| swash | bit 7 が立ち `sTypoAscender != 0` なら typo、それ以外は `hhea`。`usWin*` は使わない | `swash-0.2.9/src/metrics.rs` |
| Skia（FreeType 経路、アウトラインフォント） | bit 7 が立っていれば OS/2 typo を直接採用、それ以外は FreeType の `face->ascender` / `descender` / `height`。負の leading は 0 にする | `SkFontHost_FreeType.cpp` の `generateFontMetrics` |
| Skia（DirectWrite 経路） | GDI_CLASSIC / GDI_NATURAL の計測モードでは `GetGdiCompatibleMetrics`、それ以外では `GetMetrics`。返された `lineGap` を leading にする | `SkScalerContext_win_dw.cpp` |
| Skia（CoreText 経路） | `CTFontGetAscent` / `CTFontGetDescent` / `CTFontGetLeading` | `SkScalerContext_mac_ct.cpp` |

DirectWrite の `DWRITE_FONT_METRICS` は `ascent + descent + lineGap` を推奨行送りとし、負の line gap も許す。`DWRITE_FONT_METRICS1.hasTypographicMetrics` は、この3値がレガシー値ではなく typographic 値に基づくことを示す。ただし、bit 7 と各テーブルからの選択の全条件は、この公式 API 文書だけでは**未確認**である。CoreText API 自体の内部のテーブル選択も、ここで確認した一次資料では**未確認**である。

二次資料として、Google Fonts guide は「macOS は hhea を使い、Microsoft は bit 7 が立っていれば typo を使う（立っていなければ win）」と説明している。フォント制作側の運用指針としては広く参照されているが、上表のように描画経路ごとの実装で確かめたものではない。

Chrome の Blink は Skia の値を取得した後、指定された ascent / descent / line gap の override や丸めを適用する。WebKit の CoreText 経路は CT の3値を取得するが、MATH テーブルがあるフォントでは `tryGetTypoMetrics` が成功すると typo の値に置き換える。Gecko の macOS 経路も sfnt のメトリクス取得を優先し、そこでは HarfBuzz のメトリクス API を使う。したがって、ブラウザ名や「CoreText 使用」だけから `hhea` の採用を断定することはできない。

line gap をどこに置くかも、値の選択とは別の判断である。Ghostty の `Metrics.calc` は gap を上下に半分ずつ分配する。orzma はアセンダーの上に置くが、丸めたディセンダーをセル内に収めるため、ベースラインを引き上げることがある（`Baseline::with_line_gap`）。「デスクトップアプリは上、ブラウザは上下」という一律の規則ではない。

CSS の half-leading は `line-height - (ascent + descent)` の半分を上下に足す計算であり、OS/2 の `sTypoLineGap` を常に半分ずつ配置するという意味ではない。CSS の行ボックスの高さと、フォントから得る既定の行送りは区別する必要がある。

### 推奨される設定

仕様の recom は次の設定を挙げている。どのアプリで開いても同じ行送りになることを狙ったものである。

- `sTypoAscender - sTypoDescender = unitsPerEm` にする。
- `fsSelection` bit 7 を立てる。
- `hhea` の3値を typo の3値と同じにする。
- `usWinAscent + usWinDescent = sTypoAscender - sTypoDescender + sTypoLineGap` にする。

代案として「bit 7 を立て、`hhea.ascender = usWinAscent = sTypoAscender`、`hhea.descender = -usWinDescent = sTypoDescender`、line gap は両方 0」も挙げられている。大きなクリップ矩形が必要なら `usWin*` を大きくできるが、その場合は win で行送りを決めるアプリとの違いが残る。可変フォントでは、typo で行送りを定め bit 7 を立て、`hhea` を typo と揃え、`usWin*` はクリップ矩形として使うよう求めている。

Google Fonts の CJK 以外の新規ファミリー向けガイドは、typo = hhea、line gap = 0、`usWinAscent` / `usWinDescent` をファミリーの `yMax` / `|yMin|`、bit 7 有効、という構成を要求している。既存ファミリーの更新には行送りを維持するための例外があり、CJK は別の指針である。

## 取り消し線と下線

| 値 | テーブル | 指す位置 |
| -- | -------- | -------- |
| `yStrikeoutPosition` | OS/2 | 取り消し線の**上端**。ベースラインから上が正 |
| `yStrikeoutSize` | OS/2 | 取り消し線の太さ。0 より大きくあるべき |
| `underlinePosition` | post | 下線の**上端**の y 座標 |
| `underlineThickness` | post | 下線の太さ |

> yStrikeoutPosition: The position of the top of the strikeout stroke relative to the baseline in font design units.

> yStrikeoutSize: This field should normally be the thickness of the em dash for the current font, and should also match the underline thickness, which is specified in the 'post' table.

`yStrikeoutPosition` は em ダッシュに揃えることが勧められているが、文字の判読を妨げないよう、横棒（`e` や `H` のクロスバー）と重ならない位置にすべきとされている。

PostScript の `UnderlinePosition` は下線の**中心**を指すが、`post` テーブルはこの定義を採らない（post 仕様の注記）。FreeType は `post` の値を読むときに、太さの半分を引いて中心の位置に直している（`underline_position = underlinePosition - underlineThickness / 2`）。

Ghostty の FreeType / CoreText 経路は、取り消し線の位置と太さを OS/2 から別々に取り、`post.underlineThickness` では上書きしない。太さ 0 は欠損扱いとし、位置も 0 の場合だけ位置も欠損扱いにする。`Metrics.calc` では太さを切り上げて最低 1 px とし、セル内の位置を丸める。太さを取得できない場合は下線の太さへ退避する。orzma と比較する際は、元のフィールドだけでなく、欠損時の代替値とピクセル丸めも見る必要がある。

## その他のフィールド

- **`fsSelection`**: bit 0 ITALIC、bit 5 BOLD、bit 6 REGULAR、bit 7 USE_TYPO_METRICS、bit 8 WWS、bit 9 OBLIQUE。bit 0 と bit 5 は `head.macStyle` の bit 1 と bit 0 に一致しなければならない。bit 6 が立つときは bit 0 と bit 5 は 0 でなければならない。bit 1〜4（UNDERSCORE / NEGATIVE / OUTLINED / STRIKEOUT）はグリフ自体の装飾を表し、主に装飾用・特殊用途のフォントであることを示す。仕様はこれらを「まれにしか使われない」としている。bit 8 は name ID 21/22 なしで weight/width/slope のファミリーモデルに沿う名前を持つことを示す。bit 9 は italic と oblique を区別する処理で oblique と分類するフラグである（CSS のフォントマッチングなど）。機械的に傾けた直立書体はその一例であり、bit 0 とは独立に設定できる。
- **`fsType`**: 埋め込み許諾。bit 0 は永久予約で 0。現行定義では bit 1〜3 のうち立てるのは高々1つで、許諾部分の値は 0（Installable）、2（Restricted）、4（Preview & Print）、8（Editable）のいずれか。version 0〜2 では複数設定時に最も制限の弱い許諾を採る旧動作が認められる。bit 8 はサブセット化禁止、bit 9 はビットマップのみ埋め込み可。version 0/1 のテーブルでは bit 0〜3 しか定義されておらず、アプリケーションは bit 4〜15 を無視しなければならない。
- **`xAvgCharWidth`**: version 3 以降は幅ゼロでないグリフの単純平均。version 0〜2 では英小文字と空白の幅を指定の重みで平均する方式だった。CJK フォントでは期待値の約半分になっているものもあり、仕様は送り幅の計算に使うなと明言している。
- **`panose`**: 10バイトの外観分類。1バイト目が Family Kind で、2 = Latin Text のとき4バイト目 `bProportion` の 9 が等幅を意味する。ただし全バイト 0 のフォントもある（下の実測値を参照）。Monotype の分類ガイドはグリフ形状の比率を測り、送り幅を検証しないため、9 でも実際の等幅を保証しないと説明している。端末用の等幅判定を PANOSE だけで行うことはできない。
- **`ulUnicodeRange1-4`**: ビット割当は Unicode 5.1 に対応する OS/2 version 4（OpenType 1.5）で最後に更新された。bit 123〜127 は予約である。それ以降のスクリプトの対応表明には `meta` テーブルの `dlng` / `slng` が代替手段となる。これは対応範囲の宣言であり、個々の文字のグリフの有無は `cmap` で調べる。
- **`ulCodePageRange1-2`**: Windows のコードページ単位の対応表明。bit 17 が JIS/Japan（cp932）。
- **`sxHeight` / `sCapHeight`**: version 2 以降。デザイナーが指定できないときは、非拡大・非ヒンティングの `x`（U+0078）と `H`（U+0048）のバウンディングボックスの上端を使ってもよい。該当するグリフがなければ 0 とする。
- **`usDefaultChar` / `usBreakChar`**: BMP 外を表せないので、使用は強く非推奨。
- **`usLower/UpperOpticalPointSize`**: TWIP（1/20 pt）単位の光学サイズ範囲。STAT テーブルで置き換えられた。

## 可変フォントと MVAR

可変フォントでは、必要な OS/2 のメトリクスをインスタンスごとに MVAR テーブルの値タグで調整できる。MVAR は必須ではなく、下表の対象値は対応タグがなければ可変空間全体で一定である。

| OS/2 | タグ | OS/2 | タグ |
| ---- | ---- | ---- | ---- |
| `sTypoAscender` | `hasc` | `yStrikeoutPosition` | `stro` |
| `sTypoDescender` | `hdsc` | `yStrikeoutSize` | `strs` |
| `sTypoLineGap` | `hlgp` | `sxHeight` | `xhgt` |
| `usWinAscent` | `hcla` | `sCapHeight` | `cpht` |
| `usWinDescent` | `hcld` | `ySub*` / `ySuper*` | `sbxs` `sbys` `sbxo` `sbyo` / `spxs` `spys` `spxo` `spyo` |

`post` の下線は `undo`（position）と `unds`（thickness）で変える。`usWeightClass` と `usWidthClass` は MVAR で変えず、`wght` / `wdth` 軸の座標から導く。PANOSE はインスタンスごとに表せないので、既定インスタンスに合わせる。

## orzma での扱い

orzma は OS/2 を直接は読まず、swash の `FontRef::metrics(&[])` が返す `swash::Metrics` を経由する（`crates/bevy_orzma_tty_renderer/src/font/faces.rs`）。

セル幅・高さ・ベースライン・下線・取り消し線は primary regular face から測り、Bold / Italic / CJK fallback / symbol にも共通で使う。これらの別フェイスの OS/2 メトリクスを使ってセルや装飾を測り直すことはない。4つの primary face を比較しているのは、ASCII の右側オーバーフローである（`TerminalFonts::cell_metrics_px`）。

### swash が OS/2 から取る値

`swash-0.2.9/src/metrics.rs` の `MetricsProxy::fill` を読んだ結果。

| `swash::Metrics` | 出どころ | orzma での用途 |
| ---------------- | -------- | -------------- |
| `ascent` / `descent` / `leading` | bit 7 が立ち `sTypoAscender != 0` なら typo、それ以外は `hhea`。`descent` は入力ディセンダーの符号を反転（通常は正） | セル高と `Baseline`（`CellGeometry::measure`） |
| `strikeout_offset` | `yStrikeoutPosition`（OS/2 がなければ 0） | 取り消し線の位置（`Baseline::strike_depth`） |
| `stroke_size` | `yStrikeoutSize`。ただし **`post` があれば `post.underlineThickness` で上書き** | 取り消し線の中心への補正。`post` がある場合は下線の描画太さの元値（`Underline::measure`） |
| `underline_offset` | `post.underlinePosition` | 下線の位置 |
| `x_height` / `cap_height` | `sxHeight` / `sCapHeight`（version 2 未満なら 0） | 未使用 |
| `average_width` | `xAvgCharWidth` | 未使用（セル幅は `'0'` の送り幅から取る） |

取り消し線の中心の高さは `strikeout_offset - stroke_size / 2` で求めている。`yStrikeoutPosition` は線の上端なので、太さの半分を引けば中心になる。`post` がある場合、ここで引いているのは下線の元の太さである。一方、シェーダは `Thickness::new` で最低 1 px に補正した下線の太さで取り消し線を描くため、細い線では描画上端がフォント指定の上端と厳密には一致しない。中心の高さが 0 以下なら、OS/2 の有無にかかわらず、ピクセル換算後に丸めたアセンダーの半分だけベースラインより上に置く。

`post` がない場合、`Underline::measure` は下線の位置を `-ascent * 0.07`、太さを `ascent / 14`（最低 1 px）で代用する。取り消し線の中心補正に使う OS/2 由来の `stroke_size` と、実際に描く太さは、この場合も一致するとは限らない。

### 仕様や他の実装との差

- **MVAR は効かない。** swash は座標が空でないときだけ MVAR の差分を足す。orzma は `metrics(&[])` で呼ぶので、可変フォントは常に既定インスタンスの値になる。
- **bit 7 をバージョンで絞っていない。** 仕様は version 0〜3 の bit 7 を無視せよとしているが、swash は version を見ずに bit 7 を読む。FreeType の `sfnt_load_face` も version 4 以上という条件を設けていない。version 0〜3 では bit 7 は 0 のはずなので、この差が出るのは予約ビットに違反するフォントである。
- **`hhea` がゼロのときの退避がない。** bit 7 で typo を選ばない場合、FreeType と Ghostty は `hhea` の ascender と descender がともに 0 なら typo、さらに `usWin*` へと退避するが、swash は `hhea` の値（0）をそのまま返す。
- **`post` があると取り消し線の太さに `yStrikeoutSize` を使わない。** 上の表のとおり下線の太さになる。仕様は両者を合わせることを推奨しているが、Menlo のように一致しないフォントもある（102 と 90）。
- **選んだ行送りはグリフの最大外形とは限らない。** セル高は `floor((ascent + descent + leading) * px / UPM)` を最低 1 px にした値で、ベースラインは別に丸める。さらに別スタイルやフォールバックの縦メトリクスを参照しないため、行送りの値だけから全グリフがセル内に収まるとは保証できない。`usWin*` の GDI クリップの説明も、このレンダラーのクリップ規則ではない。

### 実測値

手元のフォントを Python の `struct` で直接読んだ値（2026-10-13、再測定して一致を確認）。同梱フォントと Fira Code は Regular の face 0、Menlo は `Menlo.ttc` の face 0（Regular）、SF は `SFNSMono.ttf` の face 0 の既定インスタンスである。

| フォント | UPM | OS/2 ver | bit 7 | hhea asc / desc / gap | typo asc / desc / gap | win asc / desc | strikeout pos / size | post ul thickness | panose[3] |
| -------- | --- | -------- | ----- | --------------------- | --------------------- | -------------- | -------------------- | ----------------- | --------- |
| JetBrainsMono Nerd Font Mono（同梱） | 1000 | 4 | ✓ | 1020 / -300 / 0 | 1020 / -300 / 0 | 1020 / 300 | 320 / 50 | 50 | 9 |
| UDEV Gothic 35（同梱） | 2048 | 4 | | 1962 / -416 / 0 | 1962 / -416 / 0 | 1962 / 416 | 655 / 102 | 102 | 3 |
| Noto Sans Symbols 2（同梱） | 1000 | 4 | ✓ | 1069 / -630 / 0 | 1069 / -630 / 0 | 1069 / 630 | 322 / 50 | 50 | 0 |
| Fira Code | 1950 | 4 | ✓ | 1800 / -600 / 0 | 1800 / -600 / 0 | 1800 / 600 | 318 / 50 | 50 | 0 |
| Menlo（macOS） | 2048 | 3 | | 1901 / -483 / 0 | 1556 / -492 / 410 | 1901 / 483 | 530 / 102 | 90 | 9 |
| .SF NS Mono Light（macOS、可変、既定） | 2048 | 4 | | 1980 / -432 / 0 | 1980 / -432 / 0 | 1980 / 432 | 600 / 100 | 100 | 9 |

読み取れること:

- この表では Menlo を除く5フォントで3組の行送りが一致している。この標本だけで最近のフォント全般の傾向までは言えない。
- **Menlo だけ typo が hhea と食い違う。** bit 7 がない（version 3 なので定義もされていない）ので、swash は hhea を使う。12 px でのセル高は hhea なら `floor(2384 × 12 / 2048) = 13`、typo なら `floor(2458 × 12 / 2048) = 14` で、1 px 変わる。
- UDEV Gothic（等幅の和文フォント）の `bProportion` は 3 で、Fira Code と Noto は PANOSE が全部 0 である。PANOSE だけで等幅かどうかは判定できない。
- 同梱の JetBrains Mono を 12 px で描くと、`depth = round(12.24) = 12`、取り消し線の中心はベースラインから `(320 - 50/2) × 12 / 1000 = 3.54` px 上で、`strike_depth = 8.46` になる。
- このとき元の線の太さは 0.6 px だが、描画太さは 1 px になる。中心を変えずに太くするので、描画上端はセル上端から 7.96 px で、フォント指定の上端 8.16 px より 0.20 px 上になる（`paint_strike` のバンド計算）。

## 参考

- [OS/2 — OS/2 and Windows metrics table](https://learn.microsoft.com/en-us/typography/opentype/spec/os2)
- [Recommendations for OpenType Fonts — Baseline to Baseline Distances](https://learn.microsoft.com/en-us/typography/opentype/spec/recom#baseline-to-baseline-distances)
- [post — PostScript Table](https://learn.microsoft.com/en-us/typography/opentype/spec/post)
- [hhea — Horizontal Header Table](https://learn.microsoft.com/en-us/typography/opentype/spec/hhea)
- [MVAR — Metrics Variations Table](https://learn.microsoft.com/en-us/typography/opentype/spec/mvar)
- [OpenType change log — Version 1.7](https://learn.microsoft.com/en-us/typography/opentype/spec/changes#version-17)
- [Apple TrueType Reference Manual — OS/2](https://developer.apple.com/fonts/TrueType-Reference-Manual/RM06/Chap6OS2.html)
- [Google Fonts Guide — Vertical Metrics](https://googlefonts.github.io/gf-guide/metrics.html)
- [Monotype PANOSE Metrics Guide — Latin Text / Proportion](https://monotype.github.io/panose/pan2.htm)
- [DirectWrite — DWRITE_FONT_METRICS](https://learn.microsoft.com/en-us/windows/win32/api/dwrite/ns-dwrite-dwrite_font_metrics)
- [DirectWrite — DWRITE_FONT_METRICS1](https://learn.microsoft.com/en-us/windows/win32/api/dwrite_1/ns-dwrite_1-dwrite_font_metrics1)
- [CSS 2.2 — Leading and half-leading](https://www.w3.org/TR/CSS22/visudet.html#leading)
- [FreeType `src/sfnt/sfobjs.c`](https://github.com/freetype/freetype/blob/master/src/sfnt/sfobjs.c)
- [Ghostty `src/font/face/freetype.zig`](https://github.com/ghostty-org/ghostty/blob/main/src/font/face/freetype.zig)
- [Ghostty `src/font/face/coretext.zig`](https://github.com/ghostty-org/ghostty/blob/main/src/font/face/coretext.zig)
- [Ghostty `src/font/Metrics.zig`](https://github.com/ghostty-org/ghostty/blob/main/src/font/Metrics.zig)
- [Skia `src/ports/SkFontHost_FreeType.cpp`](https://github.com/google/skia/blob/main/src/ports/SkFontHost_FreeType.cpp)
- [Skia `src/ports/SkScalerContext_win_dw.cpp`](https://github.com/google/skia/blob/main/src/ports/SkScalerContext_win_dw.cpp)
- [Skia `src/ports/SkScalerContext_mac_ct.cpp`](https://github.com/google/skia/blob/main/src/ports/SkScalerContext_mac_ct.cpp)
- [Blink `simple_font_data.cc`](https://github.com/chromium/chromium/blob/main/third_party/blink/renderer/platform/fonts/simple_font_data.cc)
- [WebKit `FontCoreText.cpp`](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/platform/graphics/coretext/FontCoreText.cpp)
- [Gecko `gfxMacFont.cpp`](https://github.com/mozilla/gecko-dev/blob/master/gfx/thebes/gfxMacFont.cpp)
- [Gecko `gfxFont.cpp`](https://github.com/mozilla/gecko-dev/blob/master/gfx/thebes/gfxFont.cpp)
- swash 0.2.9 `src/metrics.rs`, `src/internal/head.rs`（`~/.cargo/registry/src/` 配下）
- orzma: [faces.rs](../../crates/bevy_orzma_tty_renderer/src/font/faces.rs), [metrics.rs](../../crates/bevy_orzma_tty_renderer/src/font/metrics.rs), [geometry.rs](../../crates/bevy_orzma_tty_renderer/src/font/metrics/geometry.rs), [baseline.rs](../../crates/bevy_orzma_tty_renderer/src/font/metrics/baseline.rs), [terminal_ui_material.wesl](../../crates/bevy_orzma_tty_renderer/src/shaders/terminal_ui_material.wesl)
- 実測対象: [JetBrainsMonoNerdFontMono-Regular.ttf](../../assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Regular.ttf), [UDEVGothic35-Regular.ttf](../../crates/bevy_orzma_tty_renderer/assets/fonts/udevgothic/UDEVGothic35-Regular.ttf), [NotoSansSymbols2-Regular.ttf](../../crates/bevy_orzma_tty_renderer/assets/fonts/notosanssymbols2/NotoSansSymbols2-Regular.ttf), `~/Library/Fonts/FiraCode-Regular.ttf`, [Menlo.ttc](/System/Library/Fonts/Menlo.ttc), [SFNSMono.ttf](/System/Library/Fonts/SFNSMono.ttf)（上表の数値はこれらのファイル自体を一次資料として読んだ値）
