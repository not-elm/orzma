# `xterm-256color` 準拠の未対応一覧（Tier 1 / Tier 2）

抽出日: 2026-09-17 / 抽出元: [vt-conformance-scope.md](https://github.com/not-elm/orzma/blob/ee6a2d03b49b0d65bd6c5c648e1d09a5f49d9a90/docs/todo/vt-conformance-scope.md)
（2026-09-15 更新版。この一覧を置いたときに削除した）。各項目が `ee6a2d03` でまだ実装されていないことはコードで確認した。

Tier は元文書 §0 の基準（ncurses 6.6 の `xterm-256color` エントリ）で振り直している。
元文書で §2 に置いたままの REP と DECLRMM / DECSLRM は、この基準では Tier 1 に入る。REP は実装済み。

Alacritty も実装していない項目は、末尾の「対象外 — Alacritty も実装していないもの」に移した。

## Tier 1 — 必須（terminfo が広告しているもの）

未実装の項目はない。

## Tier 2 — 推奨（TUI が直接叩くもの）

- [ ] **`CSI ?2026 h/l` — 同期出力**
  - 2026 番の腕が無く、`SyncBuffer` は空のまま。fzf がフレームごとに発行し、nvim・tmux・kitty も使う。
- [ ] **`CSI ?Ps $ p` → `CSI ?Ps;Pm $ y` — DECRQM / DECRPM**
  - `(Some(b'?'), [b'$'], b'p')` の腕 1 本で入る。応答が無いと nvim の機能検出（69・2026 など）が必ず失敗する。
  - 7（DECAWM）と 25（DECTCEM）は報告できる状態をもう持っている。
  - 3 / 40 / 95 には `0`（not recognized）を返すと決めてある。
  - 7 に答えられるようになると、vttest の `tst_DEC_DECRPM` で DECAWM を機械判定できる。
- [ ] **`OSC 12` / `OSC 112` — カーソル色（問い合わせを含む）**
  - VT 側だけでは足りない。`Palette` のフィールド、`TerminalParams` のユニフォーム、WGSL の 3 箇所が要る。
  - 「ブロックカーソル下の文字色」と「未設定のときも反転のままにするか」を決める必要がある。
  - カーソル描画の欠陥（下の積み残しにある項目）と同じ場所なので、設定 PR とまとめるのが自然。nvim の `guicursor` が使う。

## 実装済み行の積み残し

Tier 1 / 2 の行としては実装済みだが、未対応のまま残っている部分。

### Tier 1 の行

- [ ] **フォーカス通知（1004）: `[inactive_pane]` との衝突が未裁定**
  - アプリが `FocusLost` で背景を変えたときの dim / tint との重なり方が決まっていない。
- [ ] **REP: 仕事量の増幅で全ペインが止まり得る**
  - `CSI 65535 b` は 8 バイト（8 ビットの CSI `0x9B` なら 7 バイト）で 65535 回の配置になり、REP の記憶は RIS まで消えないので、並べた出力は 4 KiB あたり約 3350 万回（8 ビットの CSI なら約 3830 万回）の配置になる。release ビルドでは、この 4 KiB の 1 チャンクの解釈に 80 列で約 0.3 秒かかる。
  - 制御機能で記憶を消す xterm 方式にしても、REP ごとに文字を 1 つ挟めば並べられるので、配置の回数は 1 割ほどしか減らない。
  - IRM を set すると、配置のたびに行の残りを右へずらすので、1 回の配置の仕事量が列数に比例する。同じ 1 チャンクが 80 列で約 2 秒、200 列で約 4 秒になる。
  - orzmux の backend スレッド 1 本が全ペインを解釈し、pump の上限は仕事量ではなくチャンク数（`MAX_CHUNKS_PER_PUMP` × `PUMP_ROUNDS`）で決まるので、1 つのペインの出力が他のペインと GUI のコマンドを止める。
  - 普通のプログラムは行幅を超える回数を送らない（ncurses は 1 行ずつ、vttest は小さな値だけ、tmux は REP を使わない）ので、既知のリスクとして受け入れた。
  - 対策は、pump の上限を経過時間で決めること。REP 以外の重い処理にも効く。
  - ただし pump が止まれるのはチャンクの境目だけなので、経過時間の上限で縮むのは 1 回の pump の停止（最大 256 チャンク分）までで、1 チャンク分の停止（上の秒数）は残る。これも縮めるには、`Interpreter::parse` に仕事量の上限を持たせてチャンクの途中で返すか、REP の残りの回数を次の `interpret` に持ち越す必要がある。

### Tier 2 の行

- [ ] **OSC 52: `?`（読み出し）**
  - VT → GUI → PTY の往復路（新しい `OrzmuxCommand`）が要る。
  - 設定ノブ（alacritty の `osc52` に相当）も読み出しと一緒に入れる。不正な OSC 52 でクリップボードがクリアされる経路にゲートを付けるかも、そこで判断する。
- [ ] **APC が CAN / SUB で途中切断されたときの誤動作**（OSC 52 の調査で判明）
  - `ESC _ Ounmount;n=<id>` が途中で切れると、全 webview がアンマウントされる。
  - `Omount;…,c=48` が `c=4` で切れると、サイズ違いの mount が通ってしまう。
- [ ] **OSC 8: ハイパーリンクの回収**
  - `HyperlinkInterner` にもレンダラの `TerminalGrid::hyperlinks` にも解放経路が無く、受信した `OSC 8` の数に比例して増え続ける。
  - 決めることは次の 4 つ: いつ解放するか、スクロールバックに残っている id との整合、`Frame` でレンダラへ削除を通知する方法、id を再利用せずに回収する方法。
- [ ] **OSC 110 / 111: 設定値へのリセット**（設定層待ち）
  - 今はハードコードされた白 / 黒に戻る。
- [ ] **OSC 11: `padding_color` の黒センチネル**（設定 PR とセット）
  - 既定背景が黒だと fallback 色に倒れるので、明示の `OSC 11;rgb:00/00/00` も同じ扱いになる。
  - この分岐は、グリッドの地・padding 帯・reverse video のグリフ色の 3 つすべてを決めている。値で兼用せず、「一度でも設定されたか」を別の信号で持つ。

## 対象外 — Alacritty も実装していないもの

照合先は alacritty `d692748d`（2026-08-31 の master）と、その `Cargo.lock` が固定する vte 0.15.0。
シーケンスの解釈は vte の `src/ansi.rs`、動作は `alacritty_terminal/src/term/mod.rs` を見た。
Alacritty 同梱の `extra/alacritty.info` も、下の terminfo capability を `sgr` 以外は広告していない。
`sgr` は広告していて、blink の引数（`%p4`）で `5` を出すが、受け側の Alacritty はそれを捨てる。
各項目の最後の行に Alacritty 側の状況を書いた。

### Tier 1

- **`CSI ?5 h/l` — DECSCNM（画面反転）**（`flash`）
  - `set_private_modes` に 5 番の腕が無い。ビジュアルベルが反応しない。
  - Alacritty: `NamedPrivateMode` に 5 番が無く、`Unknown` として無視する。
- **`CSI 5 m` — SGR blink**（`blink`, `sgr`）
  - `sgr.rs` で意図的に何もしていない（`5 | 6 | 25 | …`）。`Style` へのビット追加とレンダラの対応が要る。
  - Alacritty: vte は `Attr::BlinkSlow` / `BlinkFast` / `CancelBlink` に解釈するが、`Term::terminal_attribute` が `_ =>` の腕で捨てる。
- **`CSI ?69 h/l` / `CSI Pl;Pr s` — DECLRMM / DECSLRM**（`mgc`, `smglp`, `smglr`, `smgrp`、6.6 で広告）
  - どちらも腕が無い。nvim の矩形スクロールで要る。
  - 入れるときは、いまパラメータ無しのときだけ SCOSC として読んでいる `CSI s` を「パラメータ無しは SCOSC、ありは DECSLRM」に分ける。
  - Alacritty: 69 番が無く、`CSI s` はパラメータに関係なく SCOSC として読む。
- **`CSI > Ps q` — XTVERSION**（`XR`, `xr`、6.6 で広告）
  - 腕が無い（`(None, [b' '], b'q')` の DECSCUSR だけがある）。
  - Alacritty: `q` で受けるのは中間バイト `' '` の DECSCUSR だけで、`>` の腕が無い。
- **DA2 の応答が `rv` と一致しない**（要判断、実害は未評価）
  - 6.6 の `rv` は xterm 形の `CSI >41;…;0c` を照合するが、orzma は `CSI >0;<ver>;1c` を返す。
  - Alacritty: orzma と同じ `CSI >0;<ver>;1c` を返す（`Term::identify_terminal`）。

### Tier 2

- **`DCS $ q … ST` / `DCS + q … ST` — DECRQSS / XTGETTCAP**
  - `dcs_hook` / `dcs_put` / `dcs_unhook` が空。vim のカーソル形状の復元や capability の検出で使われる。
  - Alacritty: vte の `hook` / `put` / `unhook` はログを出すだけで、DCS をまるごと扱わない。
- **`CSI ?1015 h/l` — urxvt マウス**
  - 腕が無い。btop が 1015 → 1006 の順に発行するが、1006 があるので実害は小さい。
  - Alacritty: `NamedPrivateMode` に 1015 番が無く、`Unknown` として無視する。
- **`CSI ?Pm s` / `CSI ?Pm r` — XTSAVE / XTRESTORE**
  - 腕が無い。DECSET と同じ番号を 1 段のキャッシュで保存・復元するので、7 と 25 も対象に入る。
  - 7 を復元するときは `modes_mut` に直接書かず `DeviceState::set_auto_wrap` を通す（reset 方向で両画面の LCF を解除するため）。
  - 実際に呼ぶプログラムはまだ見つけていない。
  - Alacritty: 中間バイト `?` 付きの `s` / `r` の腕が無い。

### 実装済み行の積み残し

- **`CSI 0 T` の読み**（Tier 2 の行、観察、未修正）
  - xterm は XTHIMOUSE と読むが、orzma は SD として 1 行スクロールする（`the_scroll_down_sequence_scrolls_the_region_down` がこの挙動を固定している）。
  - Alacritty: パラメータ 0 を既定値 1 に読み替えるので、orzma と同じく SD として 1 行スクロールする。
