# DECALN - Screen Alignment Pattern

- [definition](../../docs/references/vt510.pdf#page=124)

開発用途で使用される制御関数。

## 背景

元々はCRT（ブラウン管）時代の調整用の治具。この時代の端末は画面の中心がずれる・傾く・端で歪むといった経年変化が生じ、それを目視で直すために、画面全体を均一なパターンで埋めて基準にする、という用途で利用された。

'E'という文字でスクリーン領域を塗りつぶす。

<details>
<summary>なぜ`E`で埋めるのか（AI調査）</summary>

**典拠に理由は書かれていない。** 以下は制約からの推論であり、DECが'E'を選んだ経緯の記録ではない。

- [vt220.pdf#page=48](../../docs/references/vt220.pdf#page=48) は事実のみを述べる。

> This sequence fills the screen with uppercase E's.

同節は詳細を「VT220 Pocket Service Guide」に委ねており、その文書は手元にない。

- [vt510.pdf#page=124](../../docs/references/vt510.pdf#page=124) に至っては文字を特定せず `a test pattern` としか書かない。

### 1. どの文字集合でも 'E' のまま

DECALNは保守用の道具なので、端末がどの状態にあっても同じ絵が出なければ基準にならない。この制約が最も強く効いていると考えられる。

VT端末は各国語版で文字集合が差し替わる（NRC sets）が、**差し替わるのは記号の位置だけで英大文字は不変**である。[vt220.pdf#page=67](../../docs/references/vt220.pdf#page=67) の Table 2-14: Swedish NRC Set がそれを示している。

```
8進 100 (0x40):  @  →  É     置き換わる
8進 105 (0x45):  E  →  E     置き換わらない
```

DEC Special Graphics でも同じで、`CharacterSet::graphic` の変換表が触るのは `_`（0x5F）から `~`（0x7E）までであり、A-Zは素通りする。

もしDECALNが `#` や `@` や `]` で埋める仕様だったなら、スウェーデン語版の端末では別のグリフが出て調整の基準にならない。罫線素片が指示されている状態ならなおさら成立しない。英大文字に限定される理由はここにある。

### 2. 軸に揃った直線だけでセルを埋める

26文字を「CRTの歪みを見るのに向いているか」で篩にかけると 'E' が残る。

| 候補                                | 問題                                                         |
| ----------------------------------- | ------------------------------------------------------------ |
| `O` `C` `G` `S` `Q`                 | 曲線がある。縁が歪んでいるのか元々曲線なのか区別できない     |
| `M` `W` `N` `X` `Z` `V` `A` `K` `Y` | 斜線がある。ラスタが傾いているのか字が斜めなのか判別できない |
| `B` `D` `P` `R` `J` `U`             | 右側が曲線                                                   |
| `H` `I` `T` `L` `F`                 | 軸には揃っているが線が少ない。`H` の水平線は中央の1本だけ    |
| `E`                                 | 水平3本と垂直1本。曲線も斜線もなく、セル高さを丸ごと使う     |

小文字の `e` はx-heightにしか届かずセルの縦方向を使い切らないうえ曲線でもあるため、大文字である必然性がある。

### 3. 水平線の本数がCRTでは効く

CRTは水平方向に1行ずつ走査して絵を作るので、最も起きやすく最も見つけにくい欠陥は垂直方向のリニアリティ、つまり走査線の間隔が上下で均一かどうかである。

画面を 'E' で埋めると規則正しい間隔の水平線が画面全体に敷き詰められる。人間の目は繰り返しパターンの乱れを検出するのが得意なので、間隔のわずかな詰まりが縞のうねりとして見える。'H' で埋めた場合、水平の基準線は3分の1に減る。'E' は1文字あたり3本の水平参照を供給する点で、他の軸揃い文字より優れている。

### ベタ塗り（█）ではない理由

一見すると全面塗りつぶしの方が良さそうだが逆である。

1. ASCIIに無く、VT100の文字ROMに前提として存在しない。1で述べた「どの状態でも同じ絵」が崩れる。
2. 内部構造が無いと間隔が測れない。真っ白な矩形からは外形の歪みは分かっても走査線の間隔の乱れが読めない。調整に必要なのは既知の一定ピッチで並ぶ内部構造であって面積ではない。
3. 全面高輝度はCRTの電源に負荷をかけ、輝度低下や滲みでそれ自体が幾何を歪ませる。

### 文字は選べない

`ESC # 8` は `ESC` + 中間バイト `#` + 終端 `8` の3バイト固定で、パラメータを置く場所が構文上存在しない。任意の文字で埋めるのは DECFRA（Fill Rectangular Area、[vt510.pdf#page=167](../../docs/references/vt510.pdf#page=167)）の役割であり、こちらは埋める文字・矩形範囲を指定でき、表示属性も直前のSGRに従う。DECALNが属性を既定に戻すのは、アプリの色設定に染まっては基準にならないからである。
</details>

## マージンとカーソルへの副作用

DECALNは塗りつぶしだけで終わらない。[vt510.pdf#page=124](../../docs/references/vt510.pdf#page=124) が明記している。

> Notes on DECALN
> DECALN sets the margins to the extremes of the page, and moves the cursor to the home position.

alacrittyは塗るだけでマージンにもカーソルにも触れないが、これは仕様の別解釈ではなく最小実装である。**orzmaはVT510準拠を採る。**

### 実装契約

1. origin mode（DECOM）を解除する
2. 上下マージンをページ全体へ戻す
3. カーソルをホームへ移す。`Screen::seat_home` が `pending_wrap` も同時に解除する
4. 可視画面のみを `Cell { c: 'E', ..Default::default() }` で埋める。スクロールバックには触れない
5. `DamageSpan::Full` を返す
6. pen・タブ・文字集合・checkpoint・履歴は維持する

`erase_in_display` は流用できない。あれは現在のpenの背景色で塗るBCE消去であり、DECALNは既定属性で塗る必要がある。

**典拠の強さは項目ごとに違う。2と3は上記のvt510.pdfが直接裏付けるが、1はDEC STD 070由来でローカルに典拠を持たない。**

<details>
<summary>VT510準拠を採る根拠（AI調査）</summary>

### 副作用はVT510の後付けではない

DEC社内標準 **DEC STD 070**（1985年3月18日、VT220と同時代）のAppendix D.8に、規範的な擬似コードとして存在する。

```pascal
PROCEDURE SCREEN_ALIGNMENT;
BEGIN
  ORIGIN_MODE := ABSOLUTE;
  CURRENT_RENDITION[BOLD/UNDERSCORE/BLINK/REVERSE] := FALSE;
  FOR Y := 1 TO MAX_NUM_LINES DO ...    (* 画面を埋める *)
  ACTIVE_POSITION.LINE := 1;  ACTIVE_POSITION.COLUMN := 1;
  TOP_MARGIN := 1;  BOTTOM_MARGIN := MAX_NUM_LINES;
  LAST_COLUMN_FLAG := FALSE;
END;
```

**この文書は `docs/references/` に無く、ローカルでは未検証。** 出典は bitsavers のスキャン（EL-SM070-00）。

VT100/VT102/VT220の公開マニュアルが副作用に触れないのは、「マージンを保つ」という反対規定ではなく単なる省略である。変更履歴に「DECALNが端末をリセットするという記述を削除」とあり、DECは当初より広い副作用を書いていたのを狭めた経緯であって、「触らない」方向の意図は存在しない。

### 実装調査

| 実装                                              | 上下マージン | カーソルhome | origin mode |
| ------------------------------------------------- | ------------ | ------------ | ----------- |
| xterm / Windows Terminal / mintty / VTE / ghostty | ○           | ○           | ○          |
| wezterm / kitty / foot                            | ○           | ○           | ×          |
| konsole                                           | ×           | ○           | ×          |
| alacritty                                         | ×           | ×           | ×          |

alacrittyが唯一の完全な外れ値。xtermは `charproc.c` の `CASE_DECALN` でSTD 070のページ番号をコメントに引きながら擬似コードを実装している。

alacrittyの非変更が設計判断でない根拠は、DECOMとマージンを考慮してホーム移動しdeferred wrapも解除する `goto(0,0)` が既にあり、DECSTBMはマージン設定後に明示的にそれを呼んでいること。DECCOLMやRISでは副作用を列挙している。使えるAPIがありながらDECALNでだけ使っておらず、テストも全面damageしか検証していない。

### 適合性テスト

- **esctest** は `test_DECALN_MovesCursorHome` と `test_DECALN_ClearsMargins` を持つ。alacritty方式はDECALNテスト3件中2件を落とす。
- **vttest** は依存しない。`decaln()` を画面を埋める道具として使うだけで、直後に必ず `cup()` で明示的に位置指定する。

### リスク

alacritty / kitty / wezterm のissue検索で、マージン非リセットによる破損報告は0件。terminfoの ESC # 8 ヒット0件と整合する。実害の証拠も、VT510準拠へ倒す回帰リスクの証拠も、どちらも無い。

</details>
