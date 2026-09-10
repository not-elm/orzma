# bundle 版 orzma でのみ nvim-tree の表示が崩れる件 — 調査結果

調査日: 2026-09-10 / 対象リビジョン: `6b9f0bb`

## 1. 症状

`just bundle-macos` で生成した `orzma.app` で nvim を開き、neo-tree（ファイルツリー）を開くと、
左のツリーペインに**分割前に全幅表示されていたバッファのテキストが残る**。

```
 󰉋 .claudeThe action layer: p│   1   //! The action layer: per-command ...
  .git/! grouped by domain (│   2   //! grouped by domain (vi mode: ...
```

ツリー項目名の直後に、旧テキストが**元の桁位置のまま**居座る。右側のエディタペインは正常。
`cargo run` で起動した orzma では発生しない。

## 2. 結論

**`orzma_vt` が ECH（`CSI Ps X` / Erase Character）を実装していないことが根本原因。**

`crates/orzma_vt/src/interpreter.rs` の `csi_dispatch` に `b'X'` の分岐が無く、
末尾の `_ => {}` に落ちて**黙って捨てられる**。

**debug / release の違いではない。** debug ビルドでも `TERM=xterm-256color` なら再現する。
`cargo run` で再現しなかったのは、**起動元の TERM が違っていた**ため。

## 3. 発生条件の連鎖

| # | 事象 | 根拠 |
|---|------|------|
| 1 | `.app` は launchd 起動なので `TERM` が空 | `src/main.rs:109-118` のコメントが明記 |
| 2 | 空の場合 orzma が `TERM=xterm-256color` を設定する。**空でなければ継承値をそのまま使う** | `src/main.rs:126-144` `ensure_terminfo_env` / `term_fallback` |
| 3 | ユーザーは tmux 内から `cargo run` していたため、そちらは `TERM=tmux-256color` を継承 | 実測 |
| 4 | `xterm-256color` の terminfo は `ech` を広告するが、`tmux-256color` は持たない | 下表 |
| 5 | Neovim の TUI は「画面右端に届かない矩形のクリア」に `ech` を使う。ツリーペインの余白がまさにこれ | 実測（§4.1） |
| 6 | orzma が ECH を無視 → 消えるはずのセルが残る | `interpreter.rs` の `_ => {}` |

### terminfo の差（`tput` 実測）

| capability | `xterm-256color` | `tmux-256color` | orzma 実装 |
|---|---|---|---|
| `ech` (ECH `CSI X`) | `^[[5X` | **なし** | **未実装** |
| `hpa` (CHA `CSI G`) | `^[[6G` | **なし** | **未実装** |
| `vpa` (VPA `CSI d`) | `^[[6d` | **なし** | **未実装** |
| `ich` (ICH `CSI @`) | `^[[5@` | `^[[5@` | **未実装** |
| `dch` (DCH `CSI P`) | `^[[5P` | `^[[5P` | **未実装** |
| `bce` | あり | なし | 実装相当 |
| `il1`/`dl1` | `^[[L`/`^[[M` | 同左 | 実装済（#282） |

## 4. 証拠

### 4.1 nvim の実出力を計測

Python の `pty.fork()` で 120x40 の擬似端末を作り、ユーザーの nvim 設定のまま
`nvim src/action.rs` → `Neotree show` を実行して生バイト列を取得（nvim v0.12.5）。

| | `CSI X` (ECH) | `CSI K` (EL) | サイズ |
|---|---|---|---|
| `TERM=xterm-256color`（= `.app` の条件） | **73 回** | 116 回 | 63,860 B |
| `TERM=tmux-256color`（= `cargo run` の条件） | **0 回** | 40 回 | 75,614 B |

> **注意（重要な落とし穴）**: `$TMUX` を環境に残したまま計測すると、nvim が tmux 互換
> モードに入って `TERM=xterm-256color` でも ECH を使わなくなる。最初の計測はこれで
> 誤った結論（「TERM は無関係」）に至った。**必ず `$TMUX` を unset して計測すること。**

なお nvim は `hpa`/`vpa`/`ich`/`dch`/`rep` は**どちらの TERM でも使わない**（実測 0 回）。
つまり今回の犯人は ECH 単独。

### 4.2 orzma_vt へのリプレイで bug.png を再現

取得したバイト列をそのまま `OrzmaVt` に流して grid をダンプすると、**スクリーンショットと
同一の破損が再現**した（同じ debug ビルド、変数は TERM のみ）。

```
xterm-256color:  2|  󰉋 .claudeThe action layer: p│   1   //! The action layer: ...   ← 破損
tmux-256color:   2|  󰉋 .claude                   │   1   //! The action layer: ...   ← 正常
```

### 4.3 修正の検証

ECH を実装したところ xterm 側の出力が正常な対照群と完全一致し、
**`orzma_vt` の 671 テストは全て通過**した。

## 5. 排除した仮説（すべて実証済み）

| 仮説 | 判定 | 根拠 |
|---|---|---|
| release の整数ラップ（`overflow-checks` off） | ✗ | 同一入力で debug/release の再生結果がバイト一致。加えて `cells_for` の float→int `as` は Rust 1.45 以降**サチュレート**仕様でラップし得ない |
| VT のダメージ追跡・部分フレーム・チャンク境界 | ✗ | 4 ストリーム × 7 チャンクサイズ（1 バイト刻み含む）× 2 emit ポリシー = 56 構成で全画面スナップショットと一致 |
| PTY チャンクの取りこぼし (#280) | ✗ | `try_send` が Full なら**ブロッキング `send`** にフォールバックし破棄しない（`pty.rs:372`） |
| フレームの取りこぼし（drain / observer） | ✗ | `try_iter` で順に全件処理。`differs_from` は行を含むフレームを必ず通す |
| `--no-default-features`（`debug` feature） | ✗ | macOS では CEF ローダのパスと Cmd-Q observer のみに影響 |
| 古いバンドル（#282 の IL/DL/SU/SD 修正前） | ✗ | dist は 09:42 ビルド、HEAD `6b9f0bb` は 09:36 コミット |
| フォント未同梱（`.app` に `assets/` が入らない） | ✗ | `include_bytes!` でバイナリに埋め込み済（`bundled.rs`） |
| GPU / レンダラのセル残留 | ✗ | 変更のたび `cpu_cells` を `GpuCell::default()`（`glyph_index = u32::MAX`）で埋め直して全再構築するため、未描画列は**空白**になり残像にならない |
| wide-character seam（`screen.rs:145-152` の TODO） | ✗ | 幅 2 グラフェムごとに +1 桁オーバーランするのは事実だが、症状は右シフト＋末尾切り捨てであり残像ではない。Nerd Font アイコンは PUA で `unicode-width` は幅 1 を返すのでそもそも該当しない |
| `NSHighResolutionCapable` によるスケール差 | ✗ | `build/macos/Info.plist` で `<true/>` |

## 6. 修正方針

### 6.1 ECH の実装（検証済み）

```rust
// crates/orzma_vt/src/screen.rs — erase_in_line の隣に追加
/// Erases `count` characters from the cursor rightward with the pen
/// background (BCE), leaving the cursor where it is.
///
/// The span stops at the last column, so a count past the right edge
/// erases the rest of the row rather than wrapping.
///
/// # Control Functions
///
/// - `ECH` (`CSI Pn X`)
pub fn erase_chars(&mut self, count: u16) -> Option<DamageSpan> {
    let cols = self.grid.size().cols;
    let start = self.state.column.0;
    let end = start.saturating_add(count).min(cols);
    self.grid
        .fill_visible_row_range(self.state.line, start..end, self.state.pen.erase_cell());
    self.damage_span(self.state.line, self.state.line)
}
```

```rust
// crates/orzma_vt/src/interpreter.rs — csi_dispatch の IL の手前に追加
// ECH
(None, b'X') => {
    let damage = self
        .device
        .active_screen_mut()
        .erase_chars(repeat_count(params.value(0)));
    self.stage(damage);
}
```

テストは `docs/references/` の ECH 記述を根拠に別途洗い出すこと（`/enumerate-test-cases`）。
最低限、パラメータ省略時の既定値 1、右端クランプ、pen 背景での消去（BCE）、
カーソルが動かないこと、を押さえる。

### 6.2 TERM の扱いを見直す

`xterm-256color` を名乗る以上、その terminfo が約束する能力は実装義務が生じる。
ECH は「たまたま最初に踏んだ 1 つ」に過ぎない。
**具体的な実装スコープは [vt-conformance-scope.md](vt-conformance-scope.md) に分離した。**
方針は二択:

- **A（推奨）**: 広告される命令を実装していく（§7-1）。
- **B**: 実装済み範囲に見合う TERM を名乗る。ただし独自 terminfo の配布が必要になり、
  `xterm-256color` を name しつつ未実装、という現状より運用コストは高い。

## 7. 残課題

- [ ] **1. 同じ `_ => {}` バケツに落ちている制御機能の一掃**
      → **[vt-conformance-scope.md](vt-conformance-scope.md) に Tier 1 / Tier 2 として全件洗い出し済み。**
      ECH は「たまたま最初に踏んだ 1 つ」に過ぎない。とくに **`ich`/`dch` は両方の
      terminfo が広告している**ので、`cargo run` 側でも別の TUI で同じ事故が起きる。
      実測では **DECTCEM（`CSI ?25h/l`）が 590 回**と ECH の 73 回を大きく上回る最頻出の
      未実装シーケンスだった。

- [ ] **2. `Device::resize` が alternate screen のダメージを捨てている**
      `crates/orzma_vt/src/device.rs:101-106`。`primary` のみ返し
      `let _ = self.screens.alternate.resize(size)` としている。
      `Device::reset` は `(was_showing_alternate || primary.is_some())` で正しく補正して
      いるのに `resize` はしていないため、alt screen 表示中は**別画面のダメージを返す**。
      両画面が同サイズで `Screen::resize` が `None` を返すのは `old == size` の時だけ、
      という理由で潜在化しているだけ。
      併せて `DamageSpan` に `#[must_use]` を付けて取りこぼしを検出できるようにする。

- [ ] **3. フレーム取りこぼし箇所のログ重大度が低すぎる**
      - `crates/bevy_orzmux/src/drain.rs:110-115` — `tracing::debug!("frame for an unknown pane dropped")`。
        VT は emit 時にダメージをクリアするので、これは**恒久的なロス**（今はシングル
        ペインで発火しない）。`warn!` に上げる。
      - `crates/orzma_tty_renderer/src/grid.rs:45` の silent return は、
        `insert(OrzmuxPane)` が `trigger(TtyFrameSignal)` より先にキューされるという
        **キュー順序だけ**で守られている。debug assertion を入れる。

- [ ] **4. `cell_w == 0.0` の縮退経路**
      `src/surface/geometry.rs:79-83` の `cells_for` は `inf` をサチュレートして
      **65535 桁**を返す。`material.rs:783` は自前のコピーを `.floor().max(1.0)` で
      守っているが、`cells_for` に届く `TerminalCellMetricsResource` 側の保証は未確認。

## 8. 再現手順（回帰確認用）

```bash
# 1) nvim の生出力を 2 パターン取得（$TMUX を必ず unset すること）
#    pty.fork() で 120x40 の PTY を作り、TERM を変えて以下を実行:
#    nvim src/action.rs -c 'autocmd VimEnter * ++once lua vim.defer_fn(function()
#      vim.cmd("Neotree show") vim.defer_fn(function() vim.cmd("qa!") end, 2500) end, 2500)'

# 2) ECH の出現回数を数える  →  xterm 側だけ CSI X が出れば条件成立
python3 -c "import re,sys;d=open(sys.argv[1],'rb').read();print(len(re.findall(rb'\x1b\[[0-9;]*X',d)))" capture.raw

# 3) alt screen 復帰の手前で切り、OrzmaVt に流して grid をダンプして比較
#    （末尾の ESC[?1049l より前で truncate する）
```

実アプリでの切り分けは以下が最短:

```bash
# 再現するはず（cargo run でも TERM を揃えれば出る＝プロファイル無関係の証明）
TERM=xterm-256color cargo run

# 再現しないはず（bundle 版でも TERM を揃えれば消える）
TERM=tmux-256color ./target/bundle/orzma.app/Contents/MacOS/orzma
```
