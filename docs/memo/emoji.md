# emoji

現状😱のような絵文字が表示されない。
これを表示するための実装がいくつかの実装が欠けている。

## カラー絵文字フォント

通常の文字は文字の輪郭だけを持っており色の情報などはない、カラー絵文字フォントはこの色情報を保持している。

輪郭だけのフォントは、輪郭を濃淡（カバレッジ）に変え、それを文字色で塗って描く。
`orzma`のグリッドもこの方式で、ab_glyph が輪郭を 1 px 1 バイトの濃淡にし、シェーダが fg 色で塗る。

カラー絵文字フォントは字形ごとに色そのものを持つので、文字色で塗ってはいけない。
色の持たせ方は OpenType で複数規定されていて、どれも輪郭を画素化するだけでは描けない。
COLR v0 はレイヤごとに画素化して色を付けて重ねれば描けるが、sbix / CBDT は画像の展開、COLR v1 はグラデーションや合成の実装が別に要る。

### 色の持たせ方

| テーブル | 中身 | 大きさ | 主な採用例 |
|---|---|---|---|
| `sbix` | サイズ（strike）ごとに画像を埋め込む | 固定サイズの画像を拡縮する | Apple Color Emoji |
| `CBDT` / `CBLC` | strike ごとのビットマップ（PNG など）。Google が提案した形式 | 同上 | Noto Color Emoji（CBDT 版） |
| `COLR` v0 + `CPAL` | 単色で塗った輪郭（レイヤ）を重ねる。色は`CPAL`のパレットから引く | 輪郭なので任意 | Segoe UI Emoji |
| `COLR` v1 + `CPAL` | グラデーション・変形・合成モードを持つ描画のグラフ | 同上 | Noto Color Emoji（COLRv1 版）、Segoe UI Emoji（Windows 11 の 2024 年版以降） |
| `SVG ` | 字形 ID の範囲ごとに SVG 文書を埋め込む（1 文書に複数の字形を入れられる） | 同上 | 下の OS 標準フォントは使っていない |

- ビットマップ系（sbix / CBDT）は決まった大きさの画像しか持たない。目標の大きさに近い strike を選び、拡縮して使う。
- sbix の画像の型は OpenType では`png `・`jpg `・`tiff`・`dupe`（別の字形の画像を使い回す）。Apple の仕様はさらに`pdf `・`mask`を挙げる。Apple Color Emoji は`png `に加え、仕様に無い`flip`（別の字形の画像を左右反転して使う）を持つ。
- COLR v0 はレイヤごとに「字形 ID + パレット番号」を並べたもので、各レイヤは普通の輪郭。パレット番号`0xFFFF`は文字色を意味する。
- COLR v1 はレイヤが描画の有向非巡回グラフ（同じ部品を複数から参照できる）になり、画素化に線形・放射・扇形グラデーション、アフィン変形、クリップ、合成モードの実装が要る。
- COLR v1 のテーブルは互換のため v0 のレコードも持てる。持つかどうかはフォント次第。

### OS ごとの標準フォント

| OS | フォント | 形式 | 備考 |
|---|---|---|---|
| macOS | Apple Color Emoji | sbix | `/System/Library/Fonts/Apple Color Emoji.ttc`、192 MB。face は 2 つ（0: "Apple Color Emoji"、1: ".Apple Color Emoji UI"）。strike は 20 / 26 / 32 / 40 / 48 / 52 / 64 / 96 / 160 ppem。各 strike に`flip`型の字形が 108 個ある |
| Windows 11 | Segoe UI Emoji | COLR v0 / v1 | 2024-02 に発表された版から、v0・v1・白黒の字形を同居させたハイブリッド。COLR v1 を描けないアプリは v0 か白黒を使う |
| Linux（Debian / Ubuntu / Arch） | Noto Color Emoji | CBDT | `fonts-noto-color-emoji` / `noto-fonts-emoji`。strike は 109 ppem の 1 つだけ（PNG）で、`glyf`は無い |
| Linux（Fedora 43 以降） | Noto Color Emoji | COLR v1 | システム全体の変更として COLRv1 版だけを入れるようになった。v0 のレコードは無い |

### 輪郭専用のラスタライザで描くと

Apple Color Emoji も`glyf`（輪郭）を持つが、中身は置き物で描いてもインクが出ない。
ab_glyph の`outline_glyph`に 😀 や ❤ を渡すと、字形 ID は 0 ではない（＝フォントは「持っている」と答える）のに、24 px で`alpha > 0.01`の画素が 1 つも無い（実測）。

そのため、絵文字フォントを今のフォールバックの連鎖に足すだけでは「見つかったのに空白」になり、後ろのフォントも試されない。

## 表示スタイル（presentation）

いくつかの字は、文字スタイル（白黒で、文字色で塗る）と絵文字スタイル（カラー）の 2 つの見た目を持つ。
規定は [UTS #51](https://www.unicode.org/reports/tr51/)。

### 既定：`Emoji_Presentation`

字ごとに「何も指定がなければどちらで出すか」が決まっている。

| 値 | 意味 | 例 |
|---|---|---|
| Yes | 既定で絵文字スタイル | 😀 🚀 👍 ⭐ ⌚ |
| No | 既定で文字スタイル。古い記号に多いが、追加された時期では決まらない（⌚ は Unicode 1.1 からあるが Yes、🌡 は 7.0 で追加されたが No） | ❤ ☀ © ™ ↔ ☺ 🌡 |

### 上書き：VS15 / VS16

直後に置く見えない文字（variation selector）で、既定を上書きする。

| 文字 | 意味 | 例 |
|---|---|---|
| VS15（U+FE0E） | 直前の字を文字スタイルで出す | ⌚︎（U+231A U+FE0E） |
| VS16（U+FE0F） | 直前の字を絵文字スタイルで出す | ❤️（U+2764 U+FE0F） |

- VS15 / VS16 を付けてよいのは`emoji-variation-sequences.txt`に載っている字だけ。😀 などは載っていない。
- 絵文字パレットから入力した ❤️ や ☀️ には、たいてい VS16 が付いている。VS15 はまれ。
- 要求されたスタイルの字形がどのフォントにも無いときの扱いは表示側が決める。Ghostty は最後の手段として、スタイルを問わずその字を持つフォントを使う。

### 複数の文字でできた絵文字

1 つの絵文字が複数のコードポイントでできていることがある。
どれもフォント内の合字で 1 つの字形に置き換えて描くので、シーケンス全体を扱うシェーピングが要る。
合字の表は Noto Color Emoji では GSUB、Apple Color Emoji では AAT の`morx`（GSUB は持たない）なので、シェーパは両方に対応している必要がある。

| 種類 | 構成 | 例 |
|---|---|---|
| ZWJ シーケンス | 絵文字 + U+200D + 絵文字 … | 👨‍👩‍👧 |
| 肌の色 | 絵文字 + U+1F3FB〜U+1F3FF | 👍🏻 |
| 国旗 | Regional Indicator 2 つ | 🇯🇵 |
| キーキャップ | `0-9` `#` `*` + U+FE0F + U+20E3 | 1️⃣ |
| タグシーケンス | 🏴 + タグ文字 + U+E007F | 🏴󠁧󠁢󠁳󠁣󠁴󠁿 |

### 幅

`orzma_vt`は`unicode-width` 0.2.0 で 1 文字ずつ幅を決める。

| 文字 | 幅 |
|---|---|
| U+2764 ❤ | 1 |
| U+1F600 😀 | 2 |
| U+1F3FB（肌の色） | 2 |
| U+1F1EF（Regional Indicator） | 1 |
| U+FE0F / U+FE0E | 0 |

文字列として測れば ❤️・🇯🇵・👍🏻・👨‍👩‍👧 はどれも 2 だが、1 文字ずつ足すと ❤️ = 1、👍🏻 = 4、👨‍👩‍👧 = 6 になる。

## 描画の流れ

1. 絵文字フォントを探し、cmap で字形 ID を引く。
2. 形式ごとに RGBA の画素にする。sbix / CBDT は strike を選んで PNG を展開し、目標の大きさへ拡縮する。COLR はレイヤを塗り重ねる。
3. 色の形式をそろえる。
   - 色空間: 出力の色は sRGB の値。linear 空間で計算するシェーダへ渡すなら変換が要る。しないと白っぽく薄く見える。`orzma`のアトラスは`Rgba8Unorm`で自動変換されない（bevy_text は`Rgba8UnormSrgb`を使い、サンプル時に変換させている）。
   - alpha: PNG は色と不透明度を別々に持つ（straight）。COLR を塗り重ねた結果は色に不透明度が掛かった形（premultiplied）になる。premultiplied のまま sRGB → linear 変換すると値がずれるので、一度 straight に戻してから変換し、必要なら linear で掛け直す。
   - 文字色: COLR v0 のパレット番号`0xFFFF`のレイヤは文字色で塗る。文字色に連動させるなら、キャッシュのキーに色を含めるか、そのレイヤだけ別に扱う必要がある。
4. テキストと同じアトラスに RGBA で詰め、字形ごとに「カラーかどうか」のフラグを持たせる。
5. シェーダはフラグを見て、テキストなら濃淡を文字色で塗り、カラーなら画素の色をそのまま重ねる。

描画できた字形はキャッシュされ、次からはアトラスの位置を引くだけになる。
`orzma`のアトラスのキーは face・文字・サイズ・マークで、アトラスが満杯になると全消去して描き直す。描けなかった結果はキャッシュしない。

カラー字形の扱いは実装によって分かれる。

| 実装 | アトラス | カラーかどうかの分岐 |
|---|---|---|
| Alacritty | RGBA。満杯になると同じ形式で 1 枚足す | 字形ごとのフラグを見てシェーダで分岐 |
| bevy_text | RGBA（`Rgba8UnormSrgb`）。フォントとサイズの組ごとに分かれる | 字形ごとのフラグ（`is_alpha_mask`）を見て、CPU 側でカラー字形の頂点色を白にする。シェーダは共通 |
| Ghostty / glyphon | 濃淡用とカラー用の 2 枚 | 使うアトラスで分ける |

絵文字ごとに別の`Image`を作る形は取らない。グリッドは 1 枚の UiMaterial を、フラグメントシェーダが「この画素はどのセルのどの字形か」を引きながら塗る構成で、バインドできるテクスチャの数が決まっているため。

## 他の実装

### フォントの選び方

| 実装 | 方式 |
|---|---|
| Ghostty | 直後の 1 文字が VS15 / VS16 ならそれを、無ければ`Emoji_Presentation`を希望のスタイルにする。字形がカラーか（`isColorGlyph`）が希望と合うフォントを採る。ユーザが設定したフォントは、VS が無ければスタイルを問わない。最後の手段はスタイルを問わず採る |
| WezTerm | VS15 / VS16（標準の異体字列）か、無ければクラスタ内の`Emoji_Presentation`でスタイルを決める。フォントのスタイルはフォント単位（カラーのフォントか）で、合わないフォントは飛ばす（文字スタイルならカラーフォントも飛ばす）。全滅ならスタイルを問わずやり直す |
| kitty | 幅で絵文字スタイルを判定する（幅 2 の Emoji 字は VS15 が無ければ、幅 1 の字は VS16 があれば絵文字スタイル）。絵文字スタイルなら主フォントを飛ばし、カラーを優先するフォールバックを探す |
| Alacritty | スタイルの判定は無い。OS のフォールバック列で最初に字形を持つフォントを使う |

### 大きさ

| 実装 | 方式 |
|---|---|
| Ghostty | 縦横比を保ったまま、占めるセル範囲（左右に 0.025 セルの余白）に収まる最大の大きさへ拡縮し、中央に置く（名前は cover だが CSS の contain に当たる） |
| WezTerm | ビットマップのフォントは主フォントとそのフォントのセル高さの比で拡縮し、幅が`cell_width × (num_cells + 0.25)`を超えたら縮める（拡縮できるフォントも同じ上限）。ただし縦横比 0.7 以上のフォントは、既定の設定では次のセルが空白なら縮めずにはみ出させる |
| Alacritty | フォントの自然な大きさのまま。Linux のビットマップ絵文字だけ縮小する |

### Alacritty

master `29dc553`（2026-10-05）と crossfont 0.8.1 を読んだ。

- 探索: macOS は CoreText の cascade list（言語は "en" 固定）+ "Apple Symbols"、Linux は fontconfig、Windows は DirectWrite のシステムフォールバック（crossfont `darwin/mod.rs:66-92`、`ft/mod.rs:364-466`、`directwrite/mod.rs:105-133`）。
- カラーの判定は、macOS ではフォント単位（`kCTFontColorGlyphsTrait`）、FreeType では字形単位（`FT_LOAD_COLOR`で返ったビットマップが BGRA か）。FreeType の`has_color() && !is_scalable()`は strike の選択（先頭の strike を使う）と縮小の判定に使う。
- 画素化は OS のラスタライザに任せる。CoreText は premultiplied BGRA、FreeType は`FT_LOAD_COLOR`で BGRA を返す。Windows（DirectWrite）は ClearType のマスクしか作らず、カラーにならない（`directwrite/mod.rs:82`）。
- アトラスは RGBA で、満杯になると 1 枚足す（`alacritty/src/renderer/text/atlas.rs:79`、`:247-272`）。字形に`multicolor`フラグを持たせ、シェーダはカラーのとき premultiplied を戻して画素の色を使う（`res/glsl3/text.f.glsl`）。
- VS16 は先行セルに付いた幅 0 の文字として、描画時に独立した字形の取得が試みられる。先行の字のスタイルを変える処理は無く、VS16 の字形が無ければ何も描かれない（`renderer/text/mod.rs:162-171`）。

## クレート

| crate | 何をしてくれるか | カラー形式 | 備考 |
|---|---|---|---|
| ab_glyph | 輪郭の画素化 | なし | 今の文字描画。`glyph_raster_image2`は埋め込み画像の生データ（PNG・白黒・グレー・BGRA）と配置を返すだけで、PNG の展開や COLR の画素化はしない |
| swash 0.2.9 | 字形の画素化 | sbix（`png`・`dupe`）・CBDT・COLR v0 | COLR v1・SVG・sbix の`flip`は描けない。bevy_text 経由で依存済み |
| fontique 0.9 | システムフォントの探索。`GenericFamily::Emoji`で OS の絵文字フォントを引ける | — | mmap した`Blob`を返す。bevy_text 経由で依存済み |
| icu_properties 2 | `Emoji_Presentation`などの判定 | — | parley 経由で依存済み |
| cosmic-text 0.19 | 探索・フォールバック・シェーピング・画素化 | swash と同じ | fontdb を持ち込み、fontique とフォント DB が二重になる |
| glyphon | wgpu のテキスト描画器 | swash と同じ | 独自のパイプラインで呼び出し側の RenderPass に描く。`TextAtlas`型は公開だが、中のテクスチャ（濃淡用とカラー用の 2 枚）と配置は非公開で、既存の UiMaterial には組み込めない |
| crossfont | OS ラスタライザの薄いラッパ | macOS・Linux のみ | 1 文字単位で VS を扱わない。Windows でカラーにする PR は未マージで閉じられた |
| skrifa | COLR の描画手順を`ColorPainter`で渡す | — | 画素化は自前 |
| vello_cpu 0.3 | 字形の画素化 | sbix（`png`のみ）・CBDT（PNG のみ）・COLR v0 / v1 | グリフキャッシュなど一部の API は experimental。sbix の`flip`は描けない。依存が 10〜15 増える。fontique と同じ`Blob`型を受け取る |

どの crate を使っても、RGBA アトラス、カラーフラグ、シェーダの分岐、セルに収める処理、どのフォントに回すかの判断は自前で残る。

### swash（0.2.9）の出力の癖

- `Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit)])`なら、3 形式とも同じ呼び出しで RGBA が返る。カラーの source がすべて失敗すると`None`。
- `Source::Outline`を加えると、カラーが無いときに要求した字形の普通の輪郭へ進む。Apple Color Emoji の輪郭は置き物なので空、字形 ID 0 なら .notdef の四角になる。
- カラーの source からでも、1 チャンネルのビットマップなら`Content::Mask`が返る。カラーとして扱うのは`Content::Color`のときだけにする。
- ビットマップは、strike の選び方にかかわらず要求サイズへ Mitchell フィルタで拡縮する。同じサイズなら拡縮せず、サイズ 0 なら元の大きさのまま。placement も拡縮後の値で、切り捨てのため ±1 px ずれる。
- sbix / CBDT の出力は straight alpha、COLR v0 は premultiplied に相当する。COLR v0 は整数演算のため、不透明なレイヤ 1 枚の alpha は 253 で、重なると 254 になりうる。どれも色は sRGB の値のまま。
- COLR v0 の`0xFFFF`レイヤは`Render::default_color`で塗られ、既定は灰色`[128, 128, 128, 255]`。
- family 名が "Apple Color Emoji" で、sbix の PNG の y 原点が 0 のとき、100 font units 相当（このフォントでは約 0.125 em）下へずらす。face 1 は名前が一致しないのでずらされない。
- sbix の`flip`型（Apple Color Emoji の 108 字。Unicode 15.1 の右向き ZWJ 絵文字）は描けない（[swash #147](https://github.com/dfrg/swash/issues/147)）。
- `ScaleContext`は Send + Sync。`FontRef`はバイト列を借りるだけなので、所有は`Arc`や mmap の側で持つ。
- `FontRef::from_index`を毎回呼ぶと新しい`CacheKey`が作られ、swash 内部のキャッシュが効かない。同じフォントではバイト列・face の offset・key を保持して再利用する。
- この環境で Apple Color Emoji の 3 字を 13〜52 px で描いたとき、繰り返しの平均は数十 µs 以下だった。初回はそれを超えることがある。

### メモリ

Apple Color Emoji は 192 MB ある。
fontique は mmap で開くので、`Blob`を共有で持てば RSS はほとんど増えない（10 字描いて最大 2.7 MB）。`to_vec()`でコピーすると 386 MB になる。

- `src/font/resolve.rs`の`resolve_face_bytes`は`to_vec()`するので、絵文字フォントに流用してはいけない。
- bevy_text は毎フレーム`SourceCache`を prune するので、`Blob`は自前で clone して持ち続ける。

## `orzma`の現状

- グリフの探索順は、主フォント → 同梱の UDEV Gothic35 → 同梱の Noto Sans Symbols 2（`glyph/outline.rs`の`resolve_glyph`）。主フォントの既定は同梱の JetBrains Mono Nerd で、設定で変えられる。後ろの 2 つは固定。
- Apple Color Emoji が持つ絵の字 1395 字（ASCII・VS・ZWJ・肌の色・国旗の部品を除く）のうち、既定の 3 フォントの cmap にあるのは 369 字（約 26%）で、これらは白黒で表示されうる（インクまでは測っていない）。😀 🎉 🔥 ✅ ✨ 🚀 ❌ ☺ などの 1026 字は空白になる。
- 白黒で出る字のうち Noto Sans Symbols 2 から引いたもの（👍 など）は`fit_symbol_to_cell`で 1 セル幅に縮められ、VT が 2 列を確保していても左半分に小さく描かれる。
- アトラスは 1 px 1 バイト（R8）で、GPU へは白 + 不透明度の`Rgba8Unorm`に展開している（`glyph.rs`の`expand_r8_to_rgba8`）。シェーダは不透明度だけを読んで fg で塗る。
- VT は幅を 1 文字ずつ決める。VS と ZWJ は幅 0 のマークとして直前の字に付き（1 セルに最大 9 個）、`GlyphKey`のマークにも入る。
- ただし今のマーク処理は表示スタイルを選ばない。VS も他のマークと同じく独立した輪郭として重ねようとするだけで（フォントに無ければ捨てる）、Noto Sans Symbols 2 から引いた字ではマークをすべて捨てる。

## 決まっていること

- 対象はターミナルグリッド。UI（bevy_text）の文字は対象外。
- VT の幅の決め方は変えない。ZWJ・肌の色・国旗・キーキャップは対象外で、ばらけた表示のまま。
- フォントは同梱しない。OS の絵文字フォントを使い、見つからなければ今と同じく空白にする。
- 確実に動かすのは macOS（sbix）。Windows（COLR v0）と Linux（CBDT）は動く見込みとして扱う。COLR v1 しか無い環境（Fedora 43 以降）は対象外。
- 既存の文字の描画はピクセル単位で変えない。

## 未決

- どの字を絵文字フォントで描くか（Ghostty 方式 / 今の連鎖で見つからない字だけ / Emoji 属性なら常にカラー）。
- VS16 が付いて 1 列の絵文字（❤️）を 1 セルに縮めるか、隣へはみ出させるか。
- SGR 2（faint）とブロックカーソルの下でのカラー絵文字の描き方。SGR 8（conceal）は「fg を bg と同じ色にする」今の方式が効かないので、別に隠す必要がある。
- 絵文字フォントを選ぶ・無効にする設定キーを作るか。
- 画素化に使う crate。swash を直接使う案（追加の依存なし）が有力。

## 参考

- OpenType: [sbix](https://learn.microsoft.com/en-us/typography/opentype/spec/sbix)、[CBDT](https://learn.microsoft.com/en-us/typography/opentype/spec/cbdt)、[COLR](https://learn.microsoft.com/en-us/typography/opentype/spec/colr)、[CPAL](https://learn.microsoft.com/en-us/typography/opentype/spec/cpal)、[SVG](https://learn.microsoft.com/en-us/typography/opentype/spec/svg)、[cmap format 14](https://learn.microsoft.com/en-us/typography/opentype/spec/cmap#format-14-unicode-variation-sequences)
- Apple: [morx](https://developer.apple.com/fonts/TrueType-Reference-Manual/RM06/Chap6morx.html)
- Unicode: [UTS #51 Unicode Emoji](https://www.unicode.org/reports/tr51/)、[emoji-variation-sequences.txt](https://www.unicode.org/Public/UCD/latest/ucd/emoji/emoji-variation-sequences.txt)、[emoji-data.txt](https://www.unicode.org/Public/UCD/latest/ucd/emoji/emoji-data.txt)
- [Bringing new emoji to Windows 11（Microsoft Design）](https://microsoft.design/articles/bringing-new-emoji-to-windows-11/)
- [Fedora: Use COLR for Noto Color Emoji](https://fedoraproject.org/wiki/Changes/Use_COLR_for_Noto_Color_Emoji)
- Ghostty: `src/font/CodepointResolver.zig`、`src/font/Collection.zig`、`src/font/shaper/run.zig`、`src/font/SharedGrid.zig`
- WezTerm: `wezterm-gui/src/glyphcache.rs`、`wezterm-font/src/shaper/harfbuzz.rs`、`wezterm-char-props/src/emoji.rs`
- kitty: `kitty/fonts.c`、`kitty/fontconfig.c`
