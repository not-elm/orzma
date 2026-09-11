# `xterm-256color` 準拠のための実装スコープ（Tier 1 / Tier 2）

調査日: 2026-09-10 / 対象リビジョン: `6b9f0bb`
発端: [nvim-tree-stale-cells-ech.md](nvim-tree-stale-cells-ech.md)（ECH 未実装による表示崩れ）

## 0. 判断基準

orzma は継承 `TERM` が空のとき `xterm-256color` を名乗る（`src/main.rs:126-144`）。
**名乗った以上、その terminfo エントリが広告する capability は実装契約**になる。
xterm-ctlseqs.pdf 全体の網羅は目標にしない。

- **Tier 1 = 必須** — `infocmp xterm-256color` が広告していて orzma が未実装のもの。
- **Tier 2 = 推奨** — terminfo を経由せず TUI が直接叩くもの。

### 参照章の地図（`docs/references/xterm-ctlseqs.pdf`）

| 章 | 頁 | 内容 |
|---|---|---|
| C1 (8-Bit) Control Characters | 5 | `ESC D/E/H/M/N/O/P/V/W/X/Z/[/\/]/^/_` |
| Single-character functions | 6 | BEL/BS/CR/LF/HT/SI/SO |
| Controls beginning with ESC | 7–10 | 上記**以外**の ESC 系。`ESC [` は明示的に除外 |
| **Functions using CSI** | **12–38** | ECH・ICH・DCH 等はすべてここ |
| Operating System Commands | 38–42 | OSC |

> `Controls beginning with ESC` 節の冒頭に *"This excludes controls where ESC is part of a
> 7-bit equivalent to 8-bit C1 controls"* とあり、**CSI 系はこの節に載らない**。
> スコープを ESC 章で切ると今回のバグ（ECH）を取りこぼす。

## 1. Tier 1 — 必須

未実装の落ち先は 3 箇所。表では次の略号で示す。

| 略号 | 落ち先 |
|---|---|
| `CSI∅` | `csi_dispatch` 末尾の `_ => {}`（`interpreter.rs:419`） |
| `ESC∅` | `esc_dispatch` 末尾の `_ => {}`（`interpreter.rs:222`） |
| `MODE∅` | `set_private_modes`（`interpreter.rs:587`）に番号が無い |
| `INTER∅` | intermediate 付きが dispatch 前に落ちる（`interpreter.rs:228`） |
| `OSC∅` | `osc_dispatch`（`interpreter.rs:425`）は title と cwd のみ |

### 1-A. 描画が壊れるもの（最優先）

| シーケンス | 機能 | terminfo | 現状 | 実測頻度 † | 影響 |
|---|---|---|---|---:|---|
| `CSI ?25 h/l` | DECTCEM | `civis`/`cnorm`/`cvvis` | `MODE∅`。`screen.rs:1026` の TODO でカーソルは `visible: true` 固定 | **590** | 再描画中もカーソルが本文上に残る |
| ~~`CSI Ps X`~~ | ~~**ECH**~~ | `ech` | **✅ 実装済み（2026-09-10）** | 73 | ~~消去されず旧テキストが残る~~（今回のバグ。解消済み） |
| ~~`CSI Ps @`~~ | ~~ICH~~ | `ich`, `mir` | **✅ 実装済み（2026-09-10）** | 0 | ~~挿入描画が上書きになり行が壊れる~~（解消済み） |
| ~~`CSI Ps P`~~ | ~~DCH~~ | `dch`, `dch1` | **✅ 実装済み（2026-09-10）** | 0 | ~~削除されず後続が詰まらない~~（解消済み） |
| ~~`CSI Ps G`~~ | ~~CHA~~ | `hpa` | **✅ 実装済み（2026-09-11）** | 0 | ~~桁移動が無視され以降の描画が全部ズレる~~（解消済み） |
| ~~`CSI Ps d`~~ | ~~VPA~~ | `vpa` | **✅ 実装済み（2026-09-11）** | 0 | ~~同上（行方向）~~（解消済み） |
| ~~`CSI 4 h/l`~~ | ~~IRM~~ | `smir`/`rmir`, `mir` | **✅ 実装済み（2026-09-11）**。非 private SM/RM の入口（`set_modes`）も同時に新設 | 0 | ~~挿入モードが効かず上書きになる~~（解消済み） |
| ~~`CSI ?7 h/l`~~ | ~~DECAWM~~ | `smam`/`rmam`, `am`, `xenl` | **✅ 実装済み（2026-09-11）**。`VtModes::auto_wrap` に持ち、`print` は武装・消費の両方を、EL/ECH は no-op を門番する | 0 | ~~折り返し禁止が効かず右端で溢れる／スクロールする~~（解消済み） |

† 実測頻度は「`TERM=xterm-256color`・`$TMUX` なしで nvim を起動し neo-tree を開いて終了」
までの 1 セッション（63,860 バイト）で数えた出現回数。0 は「この計測では出なかった」で
あり、他のプログラムでは出る。

### 1-B. 状態・初期化（次点）

| シーケンス | 機能 | terminfo | 現状 | 影響 |
|---|---|---|---|---|
| `CSI ! p` | DECSTR ソフトリセット | `is2`, `rs2` | `INTER∅` | **terminfo 経由の初期化列の先頭**。毎回無視されモードが残留する。`CharacterSetMapping::reset()` は DECSTR 待ちで `#[expect(dead_code)]` のまま（`character_sets.rs:220`） |
| `CSI ?12 h/l` | カーソル点滅 | `cnorm`, `cvvis` | `MODE∅`。`blinking: false` 固定 | 点滅指定が効かない |
| `CSI ?3 l` | DECCOLM リセット | `is2`, `rs2` の一部 | `MODE∅` | 初期化列に含まれる |
| `CSI ?1034 h/l` | 8bit Meta | `smm`/`rmm`, `km` | `MODE∅` | Meta キーのバイト表現が食い違う |
| `CSI ?5 h/l` | DECSCNM 反転 | `flash` | `MODE∅` | ビジュアルベルが無反応 |
| `CSI 5 m` | SGR blink | `blink`, `sgr` | **意図的に no-op**（`sgr.rs:112` の `5 \| 6 \| 25 \| ... => {}`） | 点滅が普通の文字になる。`Style` へのビット追加＋レンダラ対応が要る |
| `OSC 4;n;rgb:…` | インデックス色変更 | `initc`, `ccc` | `OSC∅` | パレット変更が効かない |

> `CSI ?4 l`（DECSCLM リセット）は広告されているが、orzma はもともとジャンプスクロール
> なので**追加で壊れる挙動は無い**。実装は不要。

### 1-C. レガシー（広告はされているが実務価値が低い）

| シーケンス | 機能 | terminfo | 判断 |
|---|---|---|---|
| `CSI 0i` / `CSI 4i` / `CSI 5i` | MC プリンタ制御 | `mc0`/`mc4`/`mc5`/`mc5i` | **無視で可**。ただし「意図的に無視」とコメントを残す |
| `ESC l` / `ESC m` | HP メモリロック | `meml`/`memu` | **無視で可**。同上 |

## 2. Tier 2 — 推奨

terminfo には出ないが実際の TUI が直接叩くもの。`—` は「ローカルの
`xterm-256color` エントリには無い」の意。

| シーケンス | 機能 | 現状 | 直接叩く実例 |
|---|---|---|---|
| `CSI Ps SP q` | DECSCUSR カーソル形状 | `INTER∅`。`Cursor` 型と `CursorShape` は既にある | **nvim が実測 5 回**（`CSI 0 q` / `1 q` / `2 q`）。vim の `term.c` |
| `CSI s` / `CSI u` | SCOSC / SCORC | `CSI∅` | blessed の `saveCursorA`/`restoreCursorA`、btop |
| `CSI ?2026 h/l` | 同期出力 | `MODE∅`。`struct SyncBuffer {}` は**空のプレースホルダ**（`interpreter.rs:83`） | fzf がフレーム毎に発行。nvim/tmux/kitty |
| `CSI ?1004` → `CSI I` / `CSI O` | フォーカス通知 | **モードは保存されるが送信側が存在しない**（`focus_in_out` の参照は定義と代入の 2 箇所のみ） | vim/nvim。フォーカス復帰時の再描画が来ない |
| `OSC 10/11/12` | 前景/背景/カーソル色（問い合わせ含む） | `OSC∅` | vim/nvim の `background` 自動判定 |
| `OSC 52` | クリップボード | `OSC∅` | nvim の osc52 provider、tmux |
| `OSC 8` | ハイパーリンク | `OSC∅`。interner は未接続（`hyperlink.rs:15`） | nvim。レンダラ側に受け皿は既にある |
| `CSI ?Ps $ p` → `$ y` | DECRQM / DECRPM | `INTER∅` | nvim が 69 や 2026 の対応可否を問い合わせる。**返answerが無いと機能検出が常に失敗する** |
| `DCS $ q … ST` / `DCS + q … ST` | DECRQSS / XTGETTCAP | DCS コールバックが空（`interpreter.rs:148`） | vim のカーソル形状復元・capability 検出 |
| ``CSI Ps ` `` / `CSI Ps a` / `CSI Ps e` | HPA / HPR / VPR | HPA は **✅ 実装済み（2026-09-11、CHA と同じメソッド）**。HPR/VPR は `CSI∅` | vttest。**VPR は `move_cursor_down` の別名にできない** — VT510 p.351 は VPR を最終行で止めるが CUD は下マージンで止まるため、DECOM リセット時にスクロール領域があると挙動が食い違う |
| `CSI Ps b` | REP | `CSI∅` | **ローカルエントリは `rep` を広告していない**ため Tier 2。vttest |
| `CSI Ps ^` | SD（ECMA-48 綴り） | `CSI∅`。orzma は `CSI T` のみ | 実際に発行するプログラムは**未確認**。安いので別名として入れる程度 |
| `CSI ?69 h/l` / `CSI Pl;Pr s` | DECLRMM / DECSLRM | `MODE∅` / `CSI∅` | nvim。矩形スクロールに必要 |
| `CSI ?1015 h/l` | urxvt マウス | `MODE∅` | btop が 1015→1006 の順に発行。1006 があるので実害は小 |

### `CSI s` の曖昧性

PDF p.30 の原文どおり、**パラメータ数ではなく DECLRMM（モード 69）の状態**で決まる。

- モード 69 **無効** → `CSI s` は SCOSC（*"Save cursor, available only when DECLRMM is disabled"*）
- モード 69 **有効** → `CSI Pl ; Pr s` は DECSLRM（*"available only when DECLRMM is enabled"*）

orzma は DECLRMM を持たない＝常に無効なので、**今は `CSI s` を素直に SCOSC にしてよい**。
将来 DECSLRM を入れるときにこの分岐を追加する。

## 3. 入力側（orzma が「送る」バイト）の契約ズレ

Tier 1/2 とは別軸。`csi_dispatch` ではなく `crates/orzma_tty/src/input/` の担当。

| capability | 広告値 | orzma の送信 | 判断 |
|---|---|---|---|
| `kbs` | `^H` (0x08) | `0x7f` (DEL) — `keyboard.rs:91` | **要判断**。entry とは食い違うが、DEL は現代の端末の事実上の標準。「ncurses の entry に合わせる」か「DEL のまま明示的に据える」かを決めて記録する |
| `kcbt` | `ESC [Z` | Shift-Tab が HT のまま | 修正対象 |
| `kich1` | `ESC [2~` | Insert キーの割り当てが無い | 修正対象 |
| `kf1`–`kf63` | `SS3 P/Q/R/S`, `CSI n ~` ほか | ファンクションキーが語彙に無い | 修正対象 |
| `kDC`/`kEND`/`kHOM`/`kLFT`/`kRIT` 等 | `CSI 1;2D` 等 | 修飾キーが落ちる（`keyboard.rs:81`） | 修正対象 |
| `kb2`/`kent` | `SS3 E` / `SS3 M` | キーパッド識別がホスト側で経路化されていない | 修正対象 |

## 4. 既存実装の疑わしい点（新規実装より先に判断が要る）

| 対象 | 内容 |
|---|---|
| **EL / ECH の pending-wrap 例外**（決着済み） | **決着: no-op を維持し、ECH も同じ方針に揃えた（2026-09-10）。参照実装が割れていることを承知した上で tmux 側を選択。** 経緯: DEC の EL 定義はアクティブ位置を含む（vt220 PDF p.36 L1754「including the cursor position」、vt510 PDF p.311 L9074「From the cursor through the end of the line」— いずれも検証済み）が、**どのマニュアルも deferred wrap をモデル化していない**ため、wrap 中にカーソルが論理的にどこに居るかを裁定しない。tmux 3.7c で実測したところ、幅10の行を埋めた状態で `CSI 0 K` も `CSI 1 X` も**何も消さず wrap も保持する**（行中では両方とも正常に動く）。tmux は `screen_write_clearcharacter` が `cx > sx - 1` で早期 return するモデル A。**訂正: 当初「alacritty も同様」と記録したが、これは誤り。** alacritty は EL と ECH を**意図的に区別している** — `alacritty_terminal-0.26.0/src/term/mod.rs:1643` の `clear_line` は `LineClearMode::Right if cursor.input_needs_wrap => return` を持つが、同 1519-1535 の `erase_chars` には `input_needs_wrap` の判定が**一切無く**、wrap 中でも最終列を消す。xterm の `CASE_ECH` も `do_wrap` を見ない。**訂正（2026-09-11）: kitty と iTerm2 も完全 no-op である。** ただし機構が違う — 両者はカーソルを `x == width` に停める方式で**ブール型のラッチを持たず**、no-op は範囲演算の帰結にすぎない（kitty は `num = MIN(columns - x, count)` が 0、iTerm2 の EL 0 は `from.x > to.x` で早期 return）。明示的なガードは iTerm2 の ECH（`cursorX >= width` で return）のみなので、「3 実装が意図的に同意している」とは言えない。なお両者は DECAWM に関係なくカーソルを停め `CSI ?7l` でも解除しないため、autowrap off で EL/ECH が永久に no-op になる危険を実際に抱えている。orzma は DECAWM 実装時にこの読み手 2 つを `auto_wrap` で門番したので、この危険は無い。**カーソル停止方式との射程合わせ（2026-09-11）**: no-op の判定は `Screen::cursor_parked_past_the_row` に集約し、ラッチ武装・`auto_wrap` 設定・**カーソルが最終列に居ること**の 3 つを要求する。3 つ目が要るのは、tmux / kitty / iTerm2 はカーソル位置そのもので判定するため no-op が行中に届かないのに対し、ブール型ラッチは `tab_to`（CBT）が右端から持ち出せてしまうため。`xterm-256color` を名乗ること、alacritty と xterm が逆であること、`docs/todo/nvim-tree-stale-cells-ech.md` §6.1 で実測検証した版にこのガードが無かったこと — これらを**承知した上で tmux 側を選択した**。実 nvim のキャプチャでは ECH は全て行中発行でこの境界を踏まないため、今回のバグ修正の妥当性には影響しない。xterm を実機で実測できた時点で再訪する価値はある |
| **1049 の pen 引き継ぎ** | `interpreter.rs:643` に「代替画面の古い pen を使う」と明記。xterm は pen を共有するので、入場時のクリアが違う背景色になり得る。BCE の正しさにも波及 |
| **DECSC/DECRC の保存範囲**（決着済み） | **決着（2026-09-11）: DECAWM は保存しない。LCF（`pending_wrap`）は保存する。** VT420 2nd ed. p.270 / VT520 p.5-120 の「Wrap flag (autowrap or no autowrap)」は LCF を指す。DEC STD-070 p.D-14 が「LCF は Save Cursor で保存し Restore Cursor で復元すべき」と明記し、xterm `cursor.c` の `DECSC_FLAGS (ATTRIBUTES\|ORIGIN\|PROTECTED)` は `WRAPAROUND` を含まない（同ファイルのコメントが VT420/VT520 の表記を DECAWM と読む解釈を逐語で却下している）。12 実装中モードを保存するのは kitty と iTerm2 の 2 つだけで、実機 VT100/220/420/510 も復元しない |
| **DA1 の応答** | entry の `u8` は `CSI ?1;2c` を期待するが `interpreter.rs:720` は `CSI ?6c`（VT102）を返す。PDF 上は許容だが、**VT102 を名乗ることで未実装の編集機能を隠してしまう**点に注意 |
| **`CSI 3 J`** | `screen.rs:105` で明示的に拒否。entry は `E3` を広告していないので Tier 1 ではないが、PDF p.13 には定義がある |
| **SGR 下線拡張** | `sgr.rs:31` が下線種別を潰し、下線色は読み捨て。vim の `58;2` 発行はリポジトリ内に既知（`sgr.rs:675`） |
| **タブストップの所有** | `tabs.rs:63` が「画面ごと」と明記。xterm は共有テーブル。PDF は所有権を規定していないので、意図的な差異として記録済み |
| **ICH が開けた桁の属性（BCE）** | vt510 p.316 は「ICH は **normal character attribute** で空白を挿入する」と規定するが、`insert_characters`（`screen.rs:526`）は `pen.erase_cell()` を使い、pen の背景を運ぶ **BCE** になっている。既存テスト `an_inserted_blank_carries_the_pen_background_without_its_rendition` が pin 済み。参照実装は割れており、xterm（`ClearCells` が `TERM_COLOR_FLAGS` で現在の fg/bg を書く）・kitty・ghostty・alacritty・foot が BCE 側、wezterm だけが `Cell::default()` で VT510 に従う。BCE は ECMA-48 にも DEC にも規定が無く、terminfo の `bce`（"screen erased with background color"）由来の概念で、しかも **erase 系**についての記述で ICH を名指ししていない。**多数派に付いた意図的な差異として記録する**（2026-09-11、IRM 実装時の調査で判明）。IRM の経路では開いた桁が直後に glyph で上書きされるため、IRM 側には影響しない |
| **`Screen::line_feed` が LCF を残す**（未修正） | 4×3 の画面で `abcd` → IND → `e` が (1,3) ではなく **(2,0)** に着地する。DEC STD-070 の LCF リセット操作一覧は LINE FEED / VERTICAL TAB / FORM FEED / INDEX / REVERSE_INDEX / NEXT_LINE を含み、xterm（`cursor.c` の `CursorDown` 末尾 `ResetWrap`）・foot（`term_linefeed` 冒頭）・Windows Terminal（`SetPosition` が無条件に `ResetDelayEOLWrap`）はいずれも解除する。同じ挙動なのは Alacritty のみ。修正は `line_feed` に 1 行だが、`screen/tests/line_feed.rs` の `a_linefeed_preserves_pending_wrap` と `a_linefeed_that_scrolls_preserves_pending_wrap` が現挙動を pin し doc も「意図的に残す」と書いているので、両テストの反転と doc 書き換えが伴う。**別 PR**。なお `tab_to` が残すのは **HT については**妥当で、Alacritty・foot・kitty がいずれも意図的に残し `wraptest` の `TAB cancels wrap` も実機 VT420/VT510 を含め大半が `n`。HT がこれで済むのは、ラッチ武装中はカーソルが必ず右端に居て `cht` が右端へクランプし、結果としてカーソルが動かないため。**CBT は別（DECAWM 実装時に判明、2026-09-11）**: `move_backward_tabs` は `tab_to` 経由でカーソルを左へ動かしつつ LCF を武装のまま残すので、「カーソルは最終列より先に居る」というラッチの前提が行中で偽になる。20 桁で実測すると `CSI 1;20H` `X` `CSI Z`（16 桁へ退避）に続く `CSI K` も `CSI 4 X` も**何も消さず**、続く印字は (0,16) ではなく **(1,0)** に着地した。消去側は `Screen::cursor_parked_past_the_row` に最終列テストを加えて修正済み（決定6 が倣った tmux / kitty / iTerm2 はラッチを持たずカーソルを `x == width` に停める方式なので、no-op が行中に届く余地がそもそも無い。その射程をラッチ実装でも再現した形）。**残るのは印字側**で、CBT のあと最初の文字がやはり次行の先頭へ行く。Alacritty も `move_backward_tabs` で `input_needs_wrap` を落とさないため同じ挙動だが、xterm・foot・Windows Terminal はカーソル移動で解除するので参照実装は割れる。**関連する未決の不整合（DECAWM 実装時に判明、2026-09-11）**: 決定6 は EL-0/ECH の no-op を autowrap-on の文脈だけに閉じたが、`Screen::erase_in_display` は LCF を読みも消しもしない。そのため autowrap が on でラッチが武装している状態では、同じカーソル位置で `CSI J` は最終列を消すのに `CSI K` は消さない、という食い違いが生じる。DEC STD-070 の LCF リセット操作一覧は ED も含んでおり、xterm も消去前に LCF を解除する。実害も実測できる: 代替画面で最終列まで埋めたあと退出し `CSI ?1049h` で再入場すると、入場時の全消去が LCF を落とさないので最初の文字が 1 行下（実測で (1,0)）に着地する。CBT（印字側）・ED・`line_feed` の 3 件は LCF リセット方針を 1 つの決定としてまとめる別 PR で一緒に裁定する |

## 5. 実装順（推奨）

1. ~~**ECH**~~ ~~**ICH/DCH**~~ **完了（2026-09-10）** → ~~**IRM**~~ **完了（2026-09-11）**。
   ECH は既存の fill だけで済み、ICH/DCH には行内スプライスの新規プリミティブ
   （`Grid::insert_visible_row_cells` / `delete_visible_row_cells`）を追加した。
   EL / ECH の pending-wrap 例外は §4 のとおり**決着済み**（no-op を維持）。
   ICH/DCH はこの例外を引き継がず、行内編集は常に deferred wrap を解除する。
   IRM は新規プリミティブを要さず、`Screen::print` が deferred wrap を解決したあと
   `insert_characters(1)` を呼ぶ形にした。この順序は入れ替えると折り返しが壊れるため
   `screen.rs` の `// NOTE:` で固定してある。モードは `VtModes::insert_replace` として
   デバイス全体で1つ持ち、代替画面切替も DECSC/DECRC も運ばない（xterm ほか8実装と一致）。
   幅2文字のシフト量は `print` の既存の幅1前提を継承しており、`screen.rs` の TODO に
   紐づく積み残し。テストは `screen/tests/print.rs` と `interpreter/tests/modes.rs`
   にあり、各 `#[test]` の doc が根拠にした仕様上の契約を持つ。
2. ~~**CHA/VPA + HPA**~~ **完了（2026-09-11）** → 次は **HPR/VPR**、**SCOSC/SCORC**、**SD `^` 別名**。
   カーソル系ヘルパ（`seat_cursor` / `seat_line` / `seat_column`）を共有。`CSI s` は将来の DECLRMM 分岐を見越した形に。
   **VPR は `move_cursor_down` の別名にできない**（§2 の注記を参照）。
3. ~~**DECAWM**~~ **完了（2026-09-11）** → 残りは **DECTCEM / カーソル点滅 / DECSCUSR**。
   `Screen::cursor()` の固定値（`screen.rs` の TODO）を実データに置き換える。
   DECSC/DECRC の保存範囲は §4 のとおり決着済み（DECAWM は保存しない）。
4. **DECSTR と初期化系**、**1049 の pen 修正**。
5. **入力側の契約修正**（`kbs` の方針決定 → Shift-Tab → ファンクションキー → 修飾キー → Meta）。
6. **OSC 4/10/11/12** とその問い合わせ・リセット。
7. **DECRQM/DECRPM と 2026 同期出力**、**DECRQSS/XTGETTCAP**。
8. **OSC 8 / OSC 52**、**DECLRMM/DECSLRM**、**1015**。
9. **残りの厳密準拠**: SGR blink、DECSCNM、メモリロック、プリンタ制御。

## 6. 検証方法

各シーケンスのテストケースは `docs/references/` を根拠に洗い出す（`/enumerate-test-cases`）。
加えて、実プログラムでの回帰は次の方法が安い:

```bash
# 1) TERM を変えて対象プログラムを PTY で走らせ、生バイト列を取る
#    （$TMUX を必ず unset すること。nvim が tmux 互換モードに入って挙動が変わる）
# 2) 当該シーケンスの出現回数を数える
python3 -c "import re,sys;d=open(sys.argv[1],'rb').read();print(len(re.findall(rb'\x1b\[[0-9;]*X',d)))" capture.raw
# 3) OrzmaVt に流して grid をダンプし、期待と突き合わせる
```

`vttest` を通すのも有効（Tier 2 の HPA/HPR/VPR/REP はいずれも vttest が直接発行する）。

`vttest` は DECAWM を検証できない。DECAWM テストはメニュー項目 2「Test of screen
features」の `tst_screen`（`main.c:620-634`）で、80 桁に**同一文字** `*` を 160 個
書いて目視確認するだけなので上書きと破棄を区別できない。`decawm(FALSE)` は他に
`vt420.c:379` の 1 箇所のみで、そこは行幅ぶんしか書かず溢れない。機械判定は
`vt320.c:715-731`（DECCIR の autowrap-pending ビット）だけで、`decawm(…)` を発行せず
電源投入時の既定を仮定している。**真の理由は DECRQM 未実装**で、`CSI ? 7 $ p` を
実装すれば `tst_DEC_DECRPM` が mode 7 を機械判定するようになる（`interpreter.rs` に
`$` = `0x24` の処理は無い）。

---

## 付記: この一覧の作り方

`infocmp xterm-256color` の boolean 8 個（`am` `bce` `ccc` `km` `mir` `msgr` `npc` `xenl`）と
文字列 capability 82 個を 1 つずつ制御機能に対応付け、orzma の `csi_dispatch` /
`esc_dispatch` / `set_private_modes` / `osc_dispatch` と突き合わせた。
Codex CLI にも独立して同じ洗い出しをさせ、両者の差分を個別に検証して統合している。
統合時に判明した訂正:

- `rep` は**このエントリには無い** → REP は Tier 1 ではなく Tier 2。
- `kbs=^H` は実在（DEL 送信は entry と不一致）。`mc5i` も広告されている。
- `acsc`（罫線）は `DecSpecialGraphics` として**実装済み**（`character_sets.rs:52`）。ギャップではない。
