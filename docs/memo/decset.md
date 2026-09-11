# DECSET / DECRST

ANSIのSM/RMに私的マーカー`?`を付けた変種。DEC由来のモード（1〜99）とXterm由来の拡張（1000番台以降）が同じ番号空間に同居している。

基本は[Xterm Control Sequences](../references/xterm-ctlseqs.pdf)、DEC由来のモードは[VT510 Video Terminal Programmer Information](../references/vt510.pdf)を参照する。

## Format

```
CSI ? Pm h    DECSET — 設定
CSI ? Pm l    DECRST — 解除
```

`Pm`はセミコロン区切りで複数指定できる。`CSI ? 1000;1006 h`は2つのモードを一度に設定する。

## 同族のシーケンス

| Sequence | Description |
| - | - |
| `CSI ? Ps $ p` | DECRQM — 問い合わせ |
| `CSI ? Ps ; Pm $ y` | DECRPM — その応答 |
| `CSI ? Pm s` | XTSAVE — 指定したモードだけを1段のキャッシュに保存 |
| `CSI ? Pm r` | XTRESTORE — 保存した値を戻す |

DECRPMの`Pm`は 0=未認識 / 1=設定 / 2=解除 / 3=恒久設定 / 4=恒久解除。**未実装のモードに0で答えられる**ので、アプリ側の機能検出が成立する。

## Orzmaが扱うモード

`VtModes`にフィールドがあり、代入するだけで済むもの。

| Ps | Description | VtModes |
| - | - | - |
| 1 | DECCKM — 矢印キーがCSIでなくSS3を送る | `app_cursor` |
| 6 | DECOM — 原点モード（**実装済み**） | — |
| 66 | DECNKM — 数値キーパッド（**実装済み**） | `keypad_mode` |
| 1000 | ボタン押下/解放を報告 | `mouse_tracking = Clicks` |
| 1002 | クリック＋ドラッグ移動 | `mouse_tracking = Drag` |
| 1003 | 全移動 | `mouse_tracking = Motion` |
| 1004 | FocusIn/FocusOut報告 | `focus_in_out` |
| 1005 | UTF-8座標拡張 | `mouse_encoding = Utf8` |
| 1006 | SGR拡張報告 | `mouse_encoding = Sgr` |
| 1007 | Alternate Scroll | `alternate_scroll` |
| 2004 | Bracketed Paste | `bracketed_paste` |

フィールドと振る舞いの追加が要るもの。

| Ps | Description | 現状 |
| - | - | - |
| 7 | DECAWM — 自動折り返し（**実装済み**） | `VtModes::auto_wrap`。**代入だけでは足りない**。`DeviceState::set_auto_wrap`を通すこと（reset側で両画面のLCFを解除する必要がある。下の節を参照） |
| 25 | DECTCEM — カーソル表示 | `Screen::cursor`にDECSCUSRと合わせて実装するTODOがある |

代替画面（**実装済み**）。単なるフラグではなく合成的な意味を持つ。

| Ps | Set | Reset |
| - | - | - |
| 47 | 代替画面へ | 通常画面へ |
| 1047 | 代替画面へ | 代替画面にいたなら**先に消去** → 通常画面へ |
| 1048 | DECSCでカーソル保存のみ | DECRCで復元のみ |
| 1049 | primaryでDECSC → 代替画面へ → **消去** | 通常画面へ → primaryでDECRC。**消去しない** |

Xterm自身がterminfoベースのアプリには47ではなくこれを使えと書いている。ctlseqsの散文は1049のresetを
「1047と1048の合成」と書くが、xtermの実装（`charproc.c`）は`?1049l`で消去しない。本実装は実装側に従う。

- **べき等**: 代替画面上での DECSET（47 / 1047 / 1049）、通常画面上での DECRST は完全な no-op
  （damageもsignalも出ず、1049のDECSC/DECRCも走らない）。alacrittyと同じ。xtermは切替が起きなくても
  CursorSave/CursorRestoreする。
- **カーソルは画面ごと**: flipは位置もpenも持ち越さない。1049の往復はprimary側の`Checkpoint`で復元される
  （xtermのDECSCスロットも`sc[whichBuf]`でバッファごと）。alt画面のcursor/pen/margins/tabsは前回の
  altセッションから残り、1049の消去はその残ったpenの背景でセルを埋める。
- 混用（`?47h`→`?1049l`など）は上のべき等規則の帰結どおりで、特別扱いしない。
  `?1049h`→`?47l`→`?1049l`では保存したカーソルは復元されない（xtermは復元する）。

## 注意点

### マウスのレベルとエンコーディング

1000/1002/1003は**互いに排他なレベル**、1005/1006/1015/1016は**直交するエンコーディング**。`MouseTracking`と`MouseEncoding`を別のenumに分けているのはこの構造による。

### 47の意味が2つある

Xtermの一覧には47として「Use Alternate Screen Buffer」と「Enable Graphic Rotated Print Mode (DECGRPM), VT340」の両方が載っている。VT340のプリンタ機能を実装しない限り実害はないが、番号空間が衝突している。

### 代替画面には履歴がない

`Screen::new(size, 0)`で構築されるため、`DeviceState::scroll`は代替画面で自然にno-opになる。

### 代替画面を離れるときのplacement退避

`DeviceState::switch_screen(Primary)`は`take_placements()`でalt画面のplacementをテーブルから外してidを返す。
**チャンク末尾の掃引（`Executor::sweep_evictions`）では拾えない**ので、`Executor::switch_screen`が発生源で
`VtSignal::WebviewEvicted`を出す。liveness はsignalではなく、flipが積む`DamageSpan::Full`が上げる。

### alt 中のリサイズと 1049 の DECRC

`DeviceState::resize`は隠れているprimaryも同じ大きさにする。primaryの`Screen::resize`は
履歴から行を取り戻す（伸長）か行を履歴へ押し出す（縮小）ので、生カーソルだけでなく
`Checkpoint`の行も同じ平行移動を受ける。そうでないと`?1049l`のDECRCが、プロンプト行より
取り戻した行数ぶん上にカーソルを置く。

### 未実装モードは黙って無視する

`set_private_modes`の`_ => {}`はそのままでよい。DECRQMを実装したときに0（未認識）で答えるのが正直な形になる。

### DECAWM は端末グローバル、LCF は画面ごと

モードは `VtModes::auto_wrap` に 1 つだけ持つ。12 実装すべてが端末グローバルで、
tmux / Alacritty / iTerm2 では「DECAWM off のまま alt 画面へ入る」を実機で確認し
3 実装とも off が共有された。一方 LCF（`pending_wrap`）は画面ごとで、これは xterm と
同じ分け方（`xw->flags` の `WRAPAROUND` がグローバル、`sc[whichBuf].wrap_flag` が
バッファごと）。

`CSI ?7l` は**両画面の live LCF を解除する。checkpoint には触らない**。reset 方向の
みなのは DEC STD-070 の LCF リセット一覧に `RESET_MODE (…AUTO_WRAP_MODE)` があり
`SET_MODE` 行には無いため（p.5-217 改訂注 16 も reset 方向のみを名指しする）。
両画面に広げるのはモードが device-global だからで、`CSI ?7;47l` のような複合指定が
順序に依存しなくなる。checkpoint に触らないのは STD-070 p.D-14 が LCF を DECSC/DECRC
で往復させると規定しているため。

`modes_mut()` 経由で `auto_wrap` を直接代入してはならない。LCF の解除が漏れ、
`?7l` → `?7h` の往復で古いラッチが折り返しに化ける。`DeviceState::set_auto_wrap`
を使う。DECSTR を実装するときにこの誘惑に当たる。

既定は ON。terminfo が `am` を広告しており、STD-070 の電源投入値
`auto_wrap_mode = WRAP_OFF; /* NVM */` と DECSTR の "Auto Wrap Off (NVM if present)"
は Set-Up 設定からの復元を意味するので、Set-Up を持たない orzma では恒久 ON に解決
される。xterm の `initflags` がまさにこの読み。したがって仕様からの逸脱ではない。
