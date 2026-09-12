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
| `CSI∅` | `csi_dispatch` 末尾の `_ => {}`（`interpreter.rs:446`） |
| `ESC∅` | `esc_dispatch` 末尾の `_ => {}`（`interpreter.rs:232`） |
| `MODE∅` | `set_private_modes`（`interpreter.rs:630`）に番号が無い |
| ~~`INTER∅`~~ | ~~intermediate 付きは `csi_dispatch` の match に届くが、腕が無く `_ => {}` に落ちる~~（DECSTR 実装時に経路を開いたことで `CSI∅` 行と同じ着地点に吸収された。2026-09-12） |
| `OSC∅` | `Executor::osc_dispatch` は title・cwd・パレット（OSC 4 / 104）のみ |

### 1-A. 描画が壊れるもの（最優先）

| シーケンス | 機能 | terminfo | 現状 | 実測頻度 † | 影響 |
|---|---|---|---|---:|---|
| ~~`CSI ?25 h/l`~~ | ~~DECTCEM~~ | `civis`/`cnorm`/`cvvis` | **✅ 実装済み（2026-09-11）**。ただし `cnorm`=`\E[?12l\E[?25h` と `cvvis`=`\E[?12;25h` は `?12` を含み、そちらは未実装なので **`cvvis` は `cnorm` と同じ定常カーソルになる**（下の `?12` 行） | **590** | ~~再描画中もカーソルが本文上に残る~~（解消済み） |
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
| `CSI ! p` | DECSTR ソフトリセット | `is2`, `rs2` | **✅ 実装済み（2026-09-12）**。`DeviceState::soft_reset` が Table 5-9 の名指しする 5 つのモード（DECTCEM / IRM / DECCKM / DECNKM / DECAWM）と**インデックス色パレット**を戻し、`Screen::soft_reset` が margins・DECOM・文字集合・pen・checkpoint を戻す。`CharacterSetMapping::reset()` の `#[expect(dead_code)]` はここで外れた。**決定 4 つ**: (1) **DECAWM は有効に戻す** — 表の "No autowrap" に従わない。`is2` は折り返しを戻すシーケンスを含まないので、表どおりだと `tput init` のたびにシェルの長い行が折り返さなくなる。xterm の DECSTR 腕は `bitcpy(&xw->flags, xw->initflags, WRAPAROUND \| …)` でリソース既定（`autoWrap` = true）に戻し、DECSTR を実装した 8 実装すべてが有効側。xterm 自身の適合性テスト `esctest2` の `test_DECSTR_DECAWM` もこの逸脱を期待値として `@intentionalDeviationFromSpec` 付きで持つ。(2) **ライブカーソルは動かさない** — xterm の `CursorSet(screen, 0, 0, …)` は `if (full)` ＝ RIS 側にしかなく、DECSTR が (0,0) に戻すのは保存カーソルのスロットだけ（`CursorSave(xw); screen->sc[whichBuf].row = col = 0;`）。vt220 Table 4-10 の脚注 `*` も "Applies only to later restore cursor commands (DECRC)" と限定する。したがって `Screen::set_scroll_region(None, None)` は `seat_home()` を含むので使えず、`Screen::soft_reset` は `scroll_region` を直接差し替える。(3) **アクティブ画面のみ** — 画面ごとの状態は DECSTR を受けた画面だけを戻す。両画面に効かせると `1049h` の入場時にプライマリの checkpoint へ保存したカーソルが消え、`1049l` での復帰が壊れる（kitty が実際に抱えている事故。Windows Terminal は GH#19918 で両画面からアクティブのみへ変更した）。(4) **パラメータは読み捨てる** — `CSI Ps ! p` を占める制御機能は無く、xterm も数を見ない。**LCF は解除しない** — (1) により `set_auto_wrap` の set 方向を通るため（§4 の LCF 一覧）。テストは `interpreter/tests/soft_reset.rs` と `screen/tests/soft_reset.rs` | ~~**terminfo 経由の初期化列の先頭**。毎回無視されモードが残留する。`CharacterSetMapping::reset()` は DECSTR 待ちで `#[expect(dead_code)]` のまま（`character_sets.rs:220`）~~ |
| `CSI ?12 h/l` | カーソル点滅 | `cnorm`, `cvvis` | `MODE∅`。`blinking: false` 固定 | 点滅指定が効かない |
| ~~`CSI ?3 h/l`~~ | ~~DECCOLM~~ | `is2`, `rs2` の一部 | **✅ 意図的に無視と明示（2026-09-12）**。`set_private_modes` に `3 => {}`。理由は `// NOTE:` に記録: ペインの幅は VT の持ち物ではなく（サイズは ウィンドウ形状 → レイアウト木 → PTY の一方通行）、vt510 p.143 が DECCOLM に定める副作用（左右上下マージンの既定化とページ全消去）だけを実行すると、来ない幅変更の代償にページを壊すことになる。`is2` に `\E[?3l` が入るので、これは **`tput init` のたびに**起きる。**xterm 自身がこのシーケンス全体を `c132` リソース（既定 off）で塞いでおり、同じ no-op に落ちる**（manpage `-132`: *"Normally, the VT102 DECCOLM escape sequence … is ignored"*、`charproc.c` の `srm_DECCOLM` は本体すべてが `if (screen->c132)` の中）。参照実装は割れている: ghostty も `?40`（既定 off）で完全無視、foot は `decset_decrst` に `case 3:` 自体が無い。kitty は **set 方向だけ**全消去＋ホーム、alacritty と wezterm は双方向でマージン既定化＋ホーム＋全消去（いずれもリサイズはしない）。テストは `interpreter/tests/column_mode.rs`（副作用を入れる変異で 4 本とも落ちることを確認済み） | ~~初期化列に含まれる~~ |
| ~~`CSI ?1034 h/l`~~ | ~~8bit Meta~~ | `smm`/`rmm`, `km` | **✅ 意図的に無視と明示（2026-09-11）**。`set_private_modes` に `1034 => {}`。Alt は常に ESC 前置（xterm の metaSendsEscape 相当）で、xterm と foot は 1036 を 1034 より優先するので、この設定では 8 ビット符号化に到達しない。bash / readline が起動時に送る `smm` は変更前から無視されており、挙動は変わらない。テストは `interpreter/tests/meta_key.rs` | ~~Meta キーのバイト表現が食い違う~~ |
| `CSI ?5 h/l` | DECSCNM 反転 | `flash` | `MODE∅` | ビジュアルベルが無反応 |
| `CSI 5 m` | SGR blink | `blink`, `sgr` | **意図的に no-op**（`sgr.rs:112` の `5 \| 6 \| 25 \| ... => {}`） | 点滅が普通の文字になる。`Style` へのビット追加＋レンダラ対応が要る |
| ~~`OSC 4;n;rgb:…`~~ | ~~インデックス色変更~~ | `initc`, `ccc`, `oc` | **✅ 実装済み（2026-09-12）**。OSC 4 の設定と `?` 問い合わせ（応答は問い合わせと同じ終端で、8 ビット値を 2 回並べた `rgb:hhhh/…`）、OSC 104（番号指定と、引数なしの全リセット）を実装し、RIS でもパレットを戻す（xterm の `ReallyReset` と同じ）。`oc=\E]104\007` は ncurses 6.6 の entry が広告する（macOS 同梱の 6.0 には無い）。**意図的に無視**: Special Colors（OSC 4 の 256〜260、OSC 5 / 105 / 6 / 106）と、`rgb:` / `#` 以外の color_spec（色名・`rgbi:`・CIE 系）。**xterm と異なる点**: 不正な組はその組だけ落として続行する（xterm は最初の誤りで打ち切る）。**制約**: vtparse 0.7 の `MAX_OSC = 64` により、1 本の OSC 4 は 31 組、OSC 104 は 63 番号まで。テストは `interpreter/tests/palette.rs` | ~~パレット変更が効かない~~ |

> `CSI ?4 l`（DECSCLM リセット）は広告されているが、orzma はもともとジャンプスクロール
> なので**追加で壊れる挙動は無い**。実装は不要。

> 観察（2026-09-12、未修正）: vtparse 0.7 は CAN / SUB で打ち切られた OSC も `osc_dispatch` する。
> ECMA-48 では CAN はシーケンスを取り消すので、取り消されたはずの `OSC 4;1;?` に応答が返る。
> 既存の title（OSC 0 / 2）も同じ挙動。

### 1-C. レガシー（広告はされているが実務価値が低い）

| シーケンス | 機能 | terminfo | 判断 |
|---|---|---|---|
| ~~`CSI 0i` / `CSI 4i` / `CSI 5i`~~ | ~~MC プリンタ制御~~ | `mc0`/`mc4`/`mc5`/`mc5i` | **✅ 意図的に無視と明示（2026-09-11）**。`csi_dispatch` に `(None \| Some(b'?'), b'i') => {}` を置き、xterm の 10/11（HTML/SVG ダンプ）と DEC private の `CSI ? Ps i`（autoprint 等）も同じ腕で塞いだ。理由は `// NOTE:` に記録: vt510 p.323 はプリンタコントローラモードを「画面に表示せずプリンタへ送る」と定めるので、忠実に実装するとプリンタ無しでは迷い込んだ `CSI 5 i` 1 つで `CSI 4 i` まで全出力が消える。**代償として `mc5i`（"printer won't echo on screen"）の広告とは食い違う**（`CSI 5 i` 以降も表示される）。alacritty も `CSI i` を持たず同じ挙動。テストは `interpreter/tests/media_copy.rs` |
| ~~`ESC l` / `ESC m`~~ | ~~HP メモリロック~~ | `meml`/`memu` | **✅ 意図的に無視と明示（2026-09-11）**。`esc_dispatch` に `(b'l' \| b'm', []) => {}`。xterm-ctlseqs p.9 は "Locks memory above the cursor" と定めるので、実装すると迷い込んだ `ESC l` でカーソルより上の行がスクロールしなくなる。テストは `interpreter/tests/memory_lock.rs`（ロックを模した変異を入れると落ちることを確認済み） |

> 1-C の 2 件はいずれも実装前から `_ => {}` に落ちて無視されていたので、**挙動は変わらない**。
> 追加したテストは変更前から通る characterization テストで、将来仕様どおりに実装した変更を検出するためのもの。

## 2. Tier 2 — 推奨

terminfo には出ないが実際の TUI が直接叩くもの。`—` は「ローカルの
`xterm-256color` エントリには無い」の意。

| シーケンス | 機能 | 現状 | 直接叩く実例 |
|---|---|---|---|
| `CSI Ps SP q` | DECSCUSR カーソル形状 | `CSI∅`（DECSTR で intermediate 経路が開いたので `(None, [b' '], b'q')` の腕 1 本で入る）。`Cursor` 型と `CursorShape` は既にある | **nvim が実測 5 回**（`CSI 0 q` / `1 q` / `2 q`）。vim の `term.c` |
| ~~`CSI s` / `CSI u`~~ | ~~SCOSC / SCORC~~ | **✅ 実装済み（2026-09-11）**。パラメータ無しのときだけ DECSC / DECRC と同じ保存枠を使う（xterm の `only_default()` に揃えた。下の「`CSI s` の曖昧性」を参照） | blessed の `saveCursorA`/`restoreCursorA`、btop |
| `CSI ?2026 h/l` | 同期出力 | `MODE∅`。`struct SyncBuffer {}` は**空のプレースホルダ**（`interpreter.rs:83`） | fzf がフレーム毎に発行。nvim/tmux/kitty |
| `CSI ?1004` → `CSI I` / `CSI O` | フォーカス通知 | **モードは保存されるが送信側が存在しない**（`focus_in_out` の参照は定義と代入の 2 箇所のみ） | vim/nvim。フォーカス復帰時の再描画が来ない |
| `OSC 10/11/12` | 前景/背景/カーソル色（問い合わせ含む） | `OSC∅` | vim/nvim の `background` 自動判定 |
| `OSC 52` | クリップボード | `OSC∅` | nvim の osc52 provider、tmux |
| `OSC 8` | ハイパーリンク | `OSC∅`。interner は未接続（`hyperlink.rs:15`） | nvim。レンダラ側に受け皿は既にある |
| `CSI ?Ps $ p` → `$ y` | DECRQM / DECRPM | `CSI∅`（DECSTR で intermediate 経路が開いたので `(Some(b'?'), [b'$'], b'p')` の腕 1 本で入る） | nvim が 69 や 2026 の対応可否を問い合わせる。**返answerが無いと機能検出が常に失敗する**。DECAWM / DECTCEM 実装により **7 と 25 も報告可能な状態を持つようになった**（`CSI ?7;1$y` / `CSI ?25;2$y` など）が応答路が無い。§6 のとおり、`CSI ?7 $ p` を実装すれば `vttest` の `tst_DEC_DECRPM` が mode 7 を機械判定できるようになる。**mode 3 / 40 / 95 は `0`（not recognized）で答える**（2026-09-12 決定）。orzma は DECCOLM の状態も変更経路も持たないので、`4`（permanently reset）だと任意幅のペインが「80 桁モード」を名乗ることになる。foot と alacritty も 0 を返す（wezterm は set を返す） |
| `DCS $ q … ST` / `DCS + q … ST` | DECRQSS / XTGETTCAP | DCS コールバックが空（`interpreter.rs:157`-`168`） | vim のカーソル形状復元・capability 検出 |
| ``CSI Ps ` `` / `CSI Ps a` / `CSI Ps e` | HPA / HPR / VPR | HPA は **✅ 実装済み（2026-09-11、CHA と同じメソッド）**。HPR も **✅ 実装済み（2026-09-11、CUF と同じメソッド。DECLRMM が無い間は停止点が一致する）**。VPR は `CSI∅` | vttest。**VPR は `move_cursor_down` の別名にできない** — VT510 p.351 は VPR を最終行で止めるが CUD は下マージンで止まるため、DECOM リセット時にスクロール領域があると挙動が食い違う |
| `CSI Ps b` | REP | `CSI∅` | **ローカルエントリは `rep` を広告していない**ため Tier 2。vttest |
| ~~`CSI Ps ^`~~ | ~~SD（xterm の別綴り）~~ | **✅ 実装済み（2026-09-11）**。ECMA-48（p.77）はこの final byte を SIMD に割り当てるが、orzma は SIMD を持たないので xterm の読み（SD）に揃えた。持つのは調べた範囲で xterm だけ（alacritty・kitty・foot・ghostty・wezterm には無い） | 実際に発行するプログラムは**未確認** |
| `CSI ?69 h/l` / `CSI Pl;Pr s` | DECLRMM / DECSLRM | `MODE∅` / `CSI∅` | nvim。矩形スクロールに必要 |
| `CSI ?1015 h/l` | urxvt マウス | `MODE∅` | btop が 1015→1006 の順に発行。1006 があるので実害は小 |
| `CSI ?Pm s` / `CSI ?Pm r` | XTSAVE / XTRESTORE | `CSI∅`（`?` 付きで intermediate 無しなので match に届いて落ちる） | xterm-ctlseqs は「DECSET と同じ Ps 値」を 1 段キャッシュで保存・復元すると規定するので、**7 と 25 も定義上この対象**。`civis`/`cnorm` の代わりに `?25 s` … `?25 r` で括るプログラムがあると hide が戻らず、`smam`/`rmam` の代わりに `?7 s` … `?7 r` で括ると autowrap が戻らない。**7 の restore は `modes_mut` 直書きにできない** — reset 方向を復元するときに両画面の LCF を解除する必要があるので `DeviceState::set_auto_wrap` を通す。具体的な呼び出し実例は未特定（低頻度と見られる） |

> 観察（2026-09-11、未修正）: xterm は `GetParam(0) == 0` の `CSI 0 T` を XTHIMOUSE と読むが、
> orzma は SD として 1 行スクロールする（`interpreter/tests/line_editing.rs` の
> `the_scroll_down_sequence_scrolls_the_region_down` が固定）。

### アプリ主導のリサイズは受けない（決着済み）

**決着（2026-09-12）: 端末サイズを変えるシーケンスは実装しない。** orzma のサイズは
ウィンドウ形状 → `OrzmuxCommand::Resize` → レイアウト木 → `OrzmaTty::resize` →
PTY ioctl + `Vt::resize` の一方通行で、VT から上流へ要求を返す経路が無い。分割ペインが
ある以上「このペインが 132 桁を要求する」はレイアウトの裁定を伴うので、経路を足すこと
自体が別規模の変更になる。

| シーケンス | 機能 | 判断 |
|---|---|---|
| `CSI ?3 h/l` | DECCOLM | §1-B のとおり明示的に無視（実装済み） |
| `CSI Ps $ \|` | DECSCPP | **実装しない。** vt510 p.143 / p.249 の Note *"It is recommended that new applications use DECSCPP rather than DECCOLM"* は主語が **applications** ＝ ホストプログラムで、**書く側への推奨であって端末実装への推奨ではない**。DECSCPP は「消さない DECCOLM」ではなく論理ページ幅そのものを 80/132 にする命令で、p.249 が定めるのは幅変更・フォント変更・新しい幅を越えたカーソルの右端クランプ・はみ出した桁のデータ破棄。幅を変えられない orzma では観測可能な効果がゼロになるので腕を置く意味が無く、`csi_dispatch` 冒頭の `has_intermediates()` ガードを外す理由にもならない（そちらは DECSTR / DECSCUSR / DECRQM が決める）。調べた範囲で実装しているのは **xterm だけ**（kitty・foot・wezterm・alacritty・ghostty にヒット無し）で、その `CASE_DECSCPP` の実体も `RequestResize` である。terminfo も広告していない |
| `CSI Pn t` / `CSI Ps * \|` | DECSLPP / DECSNLS | 同上。`CSI t` は 22 / 23（タイトルの push / pop）だけ実装済み |
| `CSI 4 t` / `CSI 8 t` | XTWINOPS リサイズ | 同上。DA1 も `CSI ?6c`（VT102）で、132 桁対応（DA1 パラメータ `1`）は名乗っていない |

> 上の一方通行を双方向にした（Vt → OrzmaTty → orzmux → レイアウト木 → ウィンドウ）
> ときに限り、DECSCPP を実装する意味が出る。そのときは DECCOLM も同時に裁定する。

### `CSI s` の曖昧性

PDF p.30 の原文は、DECLRMM（モード 69）の状態で読みを分ける。

- モード 69 **無効** → `CSI s` は SCOSC（*"Save cursor, available only when DECLRMM is disabled"*）
- モード 69 **有効** → `CSI Pl ; Pr s` は DECSLRM（*"available only when DECLRMM is enabled"*）

原文は、モード 69 が無効なときにパラメータ付きの `CSI s` をどう読むかを定めていない。**xterm の実装
（`charproc.c` の `CASE_ANSI_SC` / `CASE_ANSI_RC`）はパラメータが無いときだけ保存・復元する**
（`only_default()`）ので、orzma もそれに揃えた（2026-09-11）。kitty と wezterm も同じで、alacritty と
foot はパラメータを見ずに保存する。orzma は DECLRMM を持たないので、今はパラメータ付きの `CSI s` /
`CSI u` を無視する。将来 DECLRMM を入れるとき、この判定がそのまま「パラメータ無し＝SCOSC、あり＝DECSLRM」
の分岐になる。

## 3. 入力側（orzma が「送る」バイト）の契約ズレ

Tier 1/2 とは別軸。`csi_dispatch` ではなく `crates/orzma_tty/src/input/` の担当。

| capability | 広告値 | orzma の送信 | 判断 |
|---|---|---|---|
| `kbs` | `^H` (0x08) | `0x7f` (DEL) — `keyboard.rs:91` | **要判断**。entry とは食い違うが、DEL は現代の端末の事実上の標準。「ncurses の entry に合わせる」か「DEL のまま明示的に据える」かを決めて記録する |
| ~~`kcbt`~~ | `ESC [Z` | **✅ 修正済み（2026-09-11）**。修飾が Shift だけのときに送る。Ctrl / Alt との組み合わせは HT のまま（下の「修飾キーが落ちる」行と一緒に扱う） | 完了 |
| ~~`kich1`~~ | `ESC [2~` | **✅ 修正済み（2026-09-11）**。`TerminalKey::Insert` として編集キーパッドに加えた | 完了 |
| `kf1`–`kf63` | `SS3 P/Q/R/S`, `CSI n ~` ほか | ファンクションキーが語彙に無い | 修正対象 |
| `kDC`/`kEND`/`kHOM`/`kLFT`/`kRIT` 等 | `CSI 1;2D` 等 | 修飾キーが落ちる（`keyboard.rs:81`） | 修正対象 |
| `kb2`/`kent` | `SS3 E` / `SS3 M` | キーパッド識別がホスト側で経路化されていない | 修正対象 |

## 4. 既存実装の疑わしい点（新規実装より先に判断が要る）

| 対象 | 内容 |
|---|---|
| **EL / ECH の pending-wrap 例外**（決着済み） | **決着: no-op を維持し、ECH も同じ方針に揃えた（2026-09-10）。参照実装が割れていることを承知した上で tmux 側を選択。** 経緯: DEC の EL 定義はアクティブ位置を含む（vt220 PDF p.36 L1754「including the cursor position」、vt510 PDF p.311 L9074「From the cursor through the end of the line」— いずれも検証済み）が、**どのマニュアルも deferred wrap をモデル化していない**ため、wrap 中にカーソルが論理的にどこに居るかを裁定しない。tmux 3.7c で実測したところ、幅10の行を埋めた状態で `CSI 0 K` も `CSI 1 X` も**何も消さず wrap も保持する**（行中では両方とも正常に動く）。tmux は `screen_write_clearcharacter` が `cx > sx - 1` で早期 return するモデル A。**訂正: 当初「alacritty も同様」と記録したが、これは誤り。** alacritty は EL と ECH を**意図的に区別している** — `alacritty_terminal-0.26.0/src/term/mod.rs:1643` の `clear_line` は `LineClearMode::Right if cursor.input_needs_wrap => return` を持つが、同 1519-1535 の `erase_chars` には `input_needs_wrap` の判定が**一切無く**、wrap 中でも最終列を消す。xterm の `CASE_ECH` も `do_wrap` を見ない。**訂正（2026-09-11）: kitty と iTerm2 も完全 no-op である。** ただし機構が違う — 両者はカーソルを `x == width` に停める方式で**ブール型のラッチを持たず**、no-op は範囲演算の帰結にすぎない（kitty は `num = MIN(columns - x, count)` が 0、iTerm2 の EL 0 は `from.x > to.x` で早期 return）。明示的なガードは iTerm2 の ECH（`cursorX >= width` で return）のみなので、「3 実装が意図的に同意している」とは言えない。なお両者は DECAWM に関係なくカーソルを停め `CSI ?7l` でも解除しないため、autowrap off で EL/ECH が永久に no-op になる危険を実際に抱えている。orzma は DECAWM 実装時にこの読み手 2 つを `auto_wrap` で門番したので、この危険は無い。**カーソル停止方式との射程合わせ（2026-09-11）**: no-op の判定は `Screen::cursor_parked_past_the_row` に集約し、ラッチ武装・`auto_wrap` 設定・**カーソルが最終列に居ること**の 3 つを要求する。3 つ目が要るのは、tmux / kitty / iTerm2 はカーソル位置そのもので判定するため no-op が行中に届かないのに対し、ブール型ラッチは `tab_to`（CBT）が右端から持ち出せてしまうため。`xterm-256color` を名乗ること、alacritty と xterm が逆であること、`docs/todo/nvim-tree-stale-cells-ech.md` §6.1 で実測検証した版にこのガードが無かったこと — これらを**承知した上で tmux 側を選択した**。実 nvim のキャプチャでは ECH は全て行中発行でこの境界を踏まないため、今回のバグ修正の妥当性には影響しない。xterm を実機で実測できた時点で再訪する価値はある |
| **1049 の pen 引き継ぎ** | `interpreter.rs:693` に「代替画面の古い pen を使う」と明記。xterm は pen を共有するので、入場時のクリアが違う背景色になり得る。BCE の正しさにも波及 |
| **DECSC/DECRC の保存範囲**（決着済み） | **決着（2026-09-11）: DECAWM は保存しない。LCF（`pending_wrap`）は保存する。** VT420 2nd ed. p.270 / VT520 p.5-120 の「Wrap flag (autowrap or no autowrap)」は LCF を指す。DEC STD-070 p.D-14 が「LCF は Save Cursor で保存し Restore Cursor で復元すべき」と明記し、xterm `cursor.c` の `DECSC_FLAGS (ATTRIBUTES\|ORIGIN\|PROTECTED)` は `WRAPAROUND` を含まない（同ファイルのコメントが VT420/VT520 の表記を DECAWM と読む解釈を逐語で却下している）。12 実装中モードを保存するのは kitty と iTerm2 の 2 つだけで、実機 VT100/220/420/510 も復元しない |
| **DA1 の応答** | entry の `u8` は `CSI ?1;2c` を期待するが `interpreter.rs:770` は `CSI ?6c`（VT102）を返す。PDF 上は許容だが、**VT102 を名乗ることで未実装の編集機能を隠してしまう**点に注意 |
| **`CSI 3 J`** | `EraseScreenMode::from_ed`（`screen.rs:108`）で明示的に拒否。entry は `E3` を広告していないので Tier 1 ではないが、PDF p.13 には定義がある |
| **SGR 下線拡張** | `sgr.rs:31` が下線種別を潰し、下線色は読み捨て。vim の `58;2` 発行はリポジトリ内に既知（`sgr.rs:675`） |
| **タブストップの所有** | `tabs.rs:63` が「画面ごと」と明記。xterm は共有テーブル。PDF は所有権を規定していないので、意図的な差異として記録済み |
| **ICH が開けた桁の属性（BCE）** | vt510 p.316 は「ICH は **normal character attribute** で空白を挿入する」と規定するが、`insert_characters`（`screen.rs:549`）は `pen.erase_cell()` を使い、pen の背景を運ぶ **BCE** になっている。既存テスト `an_inserted_blank_carries_the_pen_background_without_its_rendition` が pin 済み。参照実装は割れており、xterm（`ClearCells` が `TERM_COLOR_FLAGS` で現在の fg/bg を書く）・kitty・ghostty・alacritty・foot が BCE 側、wezterm だけが `Cell::default()` で VT510 に従う。BCE は ECMA-48 にも DEC にも規定が無く、terminfo の `bce`（"screen erased with background color"）由来の概念で、しかも **erase 系**についての記述で ICH を名指ししていない。**多数派に付いた意図的な差異として記録する**（2026-09-11、IRM 実装時の調査で判明）。IRM の経路では開いた桁が直後に glyph で上書きされるため、IRM 側には影響しない |
| **`Screen::line_feed` が LCF を残す**（未修正） | 4×3 の画面で `abcd` → IND → `e` が (1,3) ではなく **(2,0)** に着地する。DEC STD-070 の LCF リセット操作一覧は LINE FEED / VERTICAL TAB / FORM FEED / INDEX / REVERSE_INDEX / NEXT_LINE を含み、xterm（`cursor.c` の `CursorDown` 末尾 `ResetWrap`）・foot（`term_linefeed` 冒頭）・Windows Terminal（`SetPosition` が無条件に `ResetDelayEOLWrap`）はいずれも解除する。同じ挙動なのは Alacritty のみ。修正は `line_feed` に 1 行だが、`screen/tests/line_feed.rs` の `a_linefeed_preserves_pending_wrap` と `a_linefeed_that_scrolls_preserves_pending_wrap` が現挙動を pin し doc も「意図的に残す」と書いているので、両テストの反転と doc 書き換えが伴う。**別 PR**。なお `tab_to` が残すのは **HT については**妥当で、Alacritty・foot・kitty がいずれも意図的に残し `wraptest` の `TAB cancels wrap` も実機 VT420/VT510 を含め大半が `n`。HT がこれで済むのは、ラッチ武装中はカーソルが必ず右端に居て `cht` が右端へクランプし、結果としてカーソルが動かないため。**CBT は別（DECAWM 実装時に判明、2026-09-11）**: `move_backward_tabs` は `tab_to` 経由でカーソルを左へ動かしつつ LCF を武装のまま残すので、「カーソルは最終列より先に居る」というラッチの前提が行中で偽になる。20 桁で実測すると `CSI 1;20H` `X` `CSI Z`（16 桁へ退避）に続く `CSI K` も `CSI 4 X` も**何も消さず**、続く印字は (0,16) ではなく **(1,0)** に着地した。消去側は `Screen::cursor_parked_past_the_row` に最終列テストを加えて修正済み（決定6 が倣った tmux / kitty / iTerm2 はラッチを持たずカーソルを `x == width` に停める方式なので、no-op が行中に届く余地がそもそも無い。その射程をラッチ実装でも再現した形）。**残るのは印字側**で、CBT のあと最初の文字がやはり次行の先頭へ行く。Alacritty も `move_backward_tabs` で `input_needs_wrap` を落とさないため同じ挙動だが、xterm・foot・Windows Terminal はカーソル移動で解除するので参照実装は割れる。**関連する未決の不整合（DECAWM 実装時に判明、2026-09-11）**: 決定6 は EL-0/ECH の no-op を autowrap-on の文脈だけに閉じたが、`Screen::erase_in_display` は LCF を読みも消しもしない。そのため autowrap が on でラッチが武装している状態では、同じカーソル位置で `CSI J` は最終列を消すのに `CSI K` は消さない、という食い違いが生じる。DEC STD-070 の LCF リセット操作一覧は ED も含んでおり、xterm も消去前に LCF を解除する。実害も実測できる: 代替画面で最終列まで埋めたあと退出し `CSI ?1049h` で再入場すると、入場時の全消去が LCF を落とさないので最初の文字が 1 行下（実測で (1,0)）に着地する。CBT（印字側）・ED・`line_feed` の 3 件は LCF リセット方針を 1 つの決定としてまとめる別 PR で一緒に裁定する |

### LCF（last column flag）をリセットする操作 — 一覧

DECAWM 実装時に §4 の4件を個別に再導出したが、これらは同じ形（カーソルを動かすか
消す操作がフラグをリセットしない）であり、DEC STD-070 は宣言的な一覧として規定して
いる。後続 PR が call site ごとに推論を繰り返さないよう、その一覧をここに置く。

STD-070 が LCF をリセットすると規定する操作:

- カーソル移動: CUU / CUD / CUF / CUB / CUP / HVP / CR / BS / **HT**、
  および **LINE FEED / VERTICAL TAB / FORM FEED / INDEX / REVERSE_INDEX / NEXT_LINE**
- 消去: **EL / ECH / ED** / DCH / ICH
- モード: **RESET_MODE (COLUMN_MODE, ORIGIN_MODE, AUTO_WRAP_MODE)** — reset 方向のみ
  （`SET_MODE` 行には `AUTO_WRAP_MODE` が無い。p.5-217 改訂注16 も reset 方向のみを名指し）
- DECSC / DECRC は LCF を**保存・復元する**（リセットしない。p.D-14）

太字は orzma の現状が STD-070 と食い違うか、参照実装が割れている箇所。orzma の現状:

| 操作 | 現状 | 備考 |
|---|---|---|
| カーソル移動（CUU/CUD/CUF/CUB/CUP/CR/BS） | リセットする | STD-070 と一致 |
| HT | **残す** | 妥当。Alacritty・foot・kitty がいずれも意図的に残し、`wraptest` の `TAB cancels wrap` も実機 VT420/VT510 を含め大半が `n` |
| CBT（`tab_to` 経由） | **残す** | 消去側は DECAWM 実装で塞いだ（`cursor_parked_past_the_row` が列も検査する）。**print 側は未修正** |
| LF / VT / FF / IND | **残す** | 未修正。xterm・foot・Windows Terminal はリセットする。Alacritty のみ orzma と同じ |
| EL / ECH | 残す（意図的） | `cursor_parked_past_the_row` が armed かつ DECAWM on かつ最終列のときだけ no-op。GNU grep バグ回避のため tmux/kitty/iTerm2 側を選択 |
| ED | **読まない** | 未修正。EL と同一カーソル位置で答えが食い違う |
| DECRST DECAWM | リセットする（両画面の live のみ） | STD-070 と一致。checkpoint には触らない |
| DECSTR | 残す（意図的） | DECAWM を**有効**に戻す決定により `set_auto_wrap` の **set 方向**を通るので、STD-070 が reset 方向にだけ課す LCF 解除は起きない。調査した実装でソフトリセット時に LCF を解除するものは 1 つも無く（xterm の DECSTR 分岐は `ResetWrap` を呼ばず、foot は hard 分岐でしか `lcf` を落とさない）、上流と整合する。`interpreter/tests/soft_reset.rs` の `a_soft_reset_leaves_an_armed_deferred_wrap_alone` が固定 |
| RIS | リセットする | `DeviceState::reset` は `VtModes::default()` を直に書くが、その前に両画面を reset するので live も checkpoint も落ちる |
| DECSC / DECRC | 保存・復元する | STD-070 p.D-14 と一致 |

後続 PR は上の表の「未修正」行をまとめて1つの方針決定として扱う。

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
2. ~~**CHA/VPA + HPA**~~ ~~**HPR**~~ ~~**SCOSC/SCORC**~~ ~~**SD `^` 別名**~~ **完了（2026-09-11）** → 残るのは **VPR**。
   カーソル系ヘルパ（`seat_cursor` / `seat_line` / `seat_column`）を共有。`CSI s` は将来の DECLRMM 分岐を見越した形に。
   **VPR は `move_cursor_down` の別名にできない**（§2 の注記を参照）。
3. ~~**DECAWM**~~ ~~**DECTCEM**~~ **両方完了（2026-09-11）**。DECTCEM は
   `VtModes::text_cursor_enable` に置き、`Screen::cursor()` が引数で受け取って
   `DeviceState::cursor()` が畳む。DECAWM は `VtModes::auto_wrap` に置き、
   `DeviceState::set_auto_wrap` 経由でのみ書く（§4 の LCF 一覧を参照）。
   残るのは **カーソル点滅（`?12`）/ DECSCUSR**。`Screen::cursor()` の固定値のうち
   `shape` と `blinking` は DECSCUSR 待ちのまま。

   **訂正**: DECTCEM 実装時に「DECAWM は `Checkpoint` に入れる必要がある」と記録したが
   これは誤りで、どちらのモードも `Checkpoint` に入らない。根拠は §4 の
   「DECSC/DECRC の保存範囲」行と `Checkpoint` の doc が持つ（重複させない）。
4. ~~**DECSTR**~~ **完了（2026-09-12）**。`DeviceState::soft_reset` は `VtModes::default()` を使わず
   5 つのフィールドを名指しで戻す（default 代入は `active_screen` を Primary に倒し、マウス・
   bracketed paste・フォーカス通知まで巻き添えにするため）。DECAWM は `DeviceState::set_auto_wrap`
   経由で **有効**に戻す（§1-B の DECSTR 行の決定 1）。ライブカーソルは動かさない（決定 2）。
   画面ごとの状態はアクティブ画面のみ（決定 3）。`DeviceState::reset_indexed_colors` も呼び、
   パレットが実際に動いたときだけ `DamageSpan::Full` を stage する。intermediate 付き CSI が
   match に届くようになったので、DECSCUSR と DECRQM は腕 1 本で入る。
   残るのは **`CSI ?12`（カーソル点滅）** と **1049 の pen 修正**。
5. **入力側の契約修正**（`kbs` の方針決定 → ファンクションキー → 修飾キー）。~~Shift-Tab~~ と Insert は **完了（2026-09-11）**。~~Meta~~ は §1-B の `CSI ?1034 h/l` 行のとおり意図的に無視と決着（2026-09-11）。
6. ~~**OSC 4**~~ **完了（2026-09-12、OSC 104 と `?` 問い合わせを含む）** → 残るのは **OSC 10/11/12** とその問い合わせ・リセット（OSC 110/111/112）。OSC 4 で入れた `PaletteRequest` を広げて扱う。RIS での復帰は `Palette::reset`（全色を既定値へ戻す）が既に賄うので、ハンドラ側は `Palette` の `foreground` / `background` を書くのと、full repaint の staging（`frame.rs` の `palette` フィールドの TODO）を足すだけでよい。なお `OSC 104` は xterm-ctlseqs.pdf のとおりインデックス表だけを戻す（`Palette::reset_all_indexed`）ので、そちらに前景/背景を巻き込まないこと。
7. **DECRQM/DECRPM と 2026 同期出力**、**DECRQSS/XTGETTCAP**。
8. **OSC 8 / OSC 52**、**DECLRMM/DECSLRM**、**1015**。
9. **残りの厳密準拠**: SGR blink、DECSCNM。~~メモリロック、プリンタ制御~~ は
   **意図的に無視と明示して完了（2026-09-11、§1-C）**。

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
