# ESC ディスパッチ — 最低限実装すべき一覧

対象は `crates/orzma_vt/src/interpreter.rs` の `Executor::esc_dispatch` と、同じ
制御機能の 8-bit C1 綴りを受ける `Executor::execute_c0_or_c1`。実装済みのものは
除いてある。

典拠は `docs/references/vt510.pdf`（ページ番号は PDF 上のもの）、
`docs/references/xterm-ctlseqs.pdf` の "Controls beginning with ESC"、および
alacritty_terminal 0.26 が ESC の解釈を委譲している `vte-0.15.0/src/ansi.rs`。

## パーサ側の前提（vtparse 0.7）

- `Escape` 状態で 0x20–0x2F は中間バイトとして収集される。`ESC SP F` は
  `intermediates = [0x20], byte = b'F'`、`ESC # 8` は `[b'#'], b'8'`、
  `ESC % G` は `[b'%'], b'G'` として届く。
- `ESC \`（ST）は DCS / OSC / APC を閉じたあと、final byte `0x5C` の
  `esc_dispatch` としてもう一度届く。
- 生の C1 バイトは `execute_c0_or_c1` に届く。ground は常に UTF-8 として復号し、
  非 UTF-8 の復号経路は存在しない。

---

## 優先度 A

### A-2. `ESC # 8` — DECALN

画面全体を `E` で埋める整列テストパターン。`Screen` に「特定の文字で埋める」操作が
無いので新しいメソッドが要る。damage は `DamageSpan::Full`。埋める範囲は画面で
あってスクロールバックではなく、表示属性は既定のまま。

実装前に決めること: VT510 は「DECALN はマージンをページの端まで広げ、カーソルを
ホーム位置へ移す」と書いているが、alacritty の `decaln` はマージンにもカーソルにも
触れない。

現在の `(b'8', [])`（DECRC）とは中間バイトで分かれるので、マッチの衝突はない。

典拠: vt510.pdf p.124。vte 0.15 は `(b'8', [b'#']) => decaln()`。

### A-3. `ESC Z`（および C1 `0x9A`）— DECID

端末 ID を返す。`CSI c`（DA1）の旧形式で、応答内容は DA1 と同一。

`Executor` の doc コメントが TODO に挙げている返信 outbox がまだ無く、DA1 自体も
`csi_dispatch` に未実装。**返す文字列の決定は DA1 と一緒に行う**（alacritty は
`\x1b[?6c`）。

`esc_dispatch` 直上の doc コメントが「8-bit C1 と 7-bit ESC の二つの綴りが乖離
しないように」と宣言しているので、`ESC Z` を足すなら `execute_c0_or_c1` に `0x9A`
のアームも同時に足す。

典拠: vt510.pdf p.89 Table 4–7。VT510 は脚注で「DECID はサポートされないことが
ある。DA1 を使え」と書いているので、優先度は DA1 の後ろでよい。

### A-5. 複数バイト終端の SCS を取りこぼしている

`ESC ( " >`（Greek, VT500）や `ESC ( % 5`（DEC Supplemental Graphics, VT300）の
ように終端が 2 バイトの SCS は `intermediates = [b'(', b'"']` の形で届くが、現在の
アームは `[designator]` という 1 要素マッチなので落ちる。

`CharacterSet::from_dscs` は「知らない終端は直前の指示を残さず ASCII へ倒す」と
doc コメントで明文化しているのに、この経路だけその意図が効かず、直前の `ESC ( 0` が
生き残って以降の文字が罫線素片として描かれる。アームを
`[designator @ (b'(' | b')' | b'*' | b'+'), ..]` に広げるだけで意図と揃う。

---

## 優先度 B

いずれも vte は実装していない。現代のアプリからの要求はほぼ無く、それぞれ相応の
下地を要求する。

### B-1. `ESC 6` / `ESC 9` — DECBI / DECFI

カーソルを 1 桁戻す／進める。左（右）マージンにいる場合はマージン内の画面データが
1 桁右（左）へずれ、押し出された桁は失われる。ページの端では無視。

必要な下地は列単位の挿入・削除で、CSI 側の DECIC / DECDC と同じ機構になる。

典拠: vt510.pdf p.130 / p.163。

### B-2. `ESC SP F` / `ESC SP G` — S7C1T / S8C1T

端末がホストへ返す C1 制御を 7-bit エスケープで送るか 8-bit 単一文字で送るかの
選択。入力の解釈ではなく**応答の符号化**を変える。

orzma の応答は 7-bit で組む前提なので、S7C1T は現状と一致する no-op、S8C1T は
outbox が 8-bit C1 を吐けるようになって初めて意味を持つ。当面はどちらも no-op。

典拠: vt510.pdf p.333 / p.334。

### B-3. `ESC ~` / `ESC }` / `ESC |` — LS1R / LS2R / LS3R

G1 / G2 / G3 を GR（0xA0–0xFF）へ locking shift する。`CharacterSetMapping` に GR の
概念が無い（今あるのは `gl: GCode` だけ）ので、GL/GR の二本立てに広げる必要がある。

ただし ground が常に UTF-8 で復号する以上、単独の 0xA0–0xFF が翻訳器に届くことは
なく、GR は事実上死んでいる。GL 側の LS2 / LS3 が実装済みなのは `ESC n` / `ESC o` が
7-bit 経路で意味を持つから、という非対称は妥当。

典拠: vt510.pdf p.95 の locking shift 表。

### B-4. `ESC # 3` / `# 4` / `# 5` / `# 6` — DECDHL / DECSWL / DECDWL

行単位の倍幅・倍高属性。`# 3` が倍高の上半分、`# 4` が下半分、`# 5` が単幅単高
（既定）、`# 6` が倍幅単高。

この一覧で唯一**レンダラ側の作業**を要求する。行属性を `Row` に持たせたうえで、
`orzma_tty_renderer` がグリフを拡大して描く必要がある。VT の中だけでは閉じないので、
入れるかどうかはプロダクトの判断。

典拠: vt510.pdf p.147 / p.157 / p.280。

---

## 見送り

| Sequence                  | 名前                  | 理由                                                                       |
| ------------------------- | --------------------- | -------------------------------------------------------------------------- |
| `ESC V` / `ESC W`         | SPA / EPA             | 選択的消去（DECSCA / DECSED / DECSEL）と組でなければ意味を持たない         |
| `ESC F`                   | カーソルを左下隅へ    | HP 互換。xterm も `hpLowerleftBugCompat` を有効にしたときだけ動く          |
| `ESC l` / `ESC m`         | Memory Lock / Unlock  | HP 端末互換。orzma のスクロールバック模型に対応物がない                    |
| `ESC - C` / `. C` / `/ C` | 96 文字集合の指示     | `GCode::from_designator` が意図的に `None` を返す（全て 94 文字集合のため） |
| `ESC SP L` / `M` / `N`    | ANSI conformance level | orzma は単一レベルで動く                                                   |
| VT52 モードの ESC 群      | —                     | `DECANM`（`CSI ? 2 l`）を実装しない限り入口が無い                          |
| Tektronix 4014 の ESC 群  | —                     | エミュレーション対象外                                                     |
