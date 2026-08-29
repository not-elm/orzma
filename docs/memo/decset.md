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
| 7 | DECAWM — 自動折り返し | `Screen::print`が右端で無条件に`pending_wrap`を立てるため常時オン相当 |
| 25 | DECTCEM — カーソル表示 | `Screen::cursor`にDECSCUSRと合わせて実装するTODOがある |

代替画面。単なるフラグではなく合成的な意味を持つ。

| Ps | Set | Reset |
| - | - | - |
| 47 | 代替画面へ | 通常画面へ |
| 1047 | 代替画面へ | 通常画面へ。**代替画面にいたなら先に消去** |
| 1048 | DECSCでカーソル保存のみ | DECRCで復元のみ |
| 1049 | カーソル保存 → 代替画面へ → **消去** | 通常画面へ → カーソル復元 |

1049は1047と1048の合成。Xterm自身がterminfoベースのアプリには47ではなくこれを使えと書いている。

## 注意点

### マウスのレベルとエンコーディング

1000/1002/1003は**互いに排他なレベル**、1005/1006/1015/1016は**直交するエンコーディング**。`MouseTracking`と`MouseEncoding`を別のenumに分けているのはこの構造による。

### 47の意味が2つある

Xtermの一覧には47として「Use Alternate Screen Buffer」と「Enable Graphic Rotated Print Mode (DECGRPM), VT340」の両方が載っている。VT340のプリンタ機能を実装しない限り実害はないが、番号空間が衝突している。

### 代替画面には履歴がない

`Screen::new(size, 0)`で構築されるため、`DeviceState::scroll`は代替画面で自然にno-opになる。

### 1049実装時のplacement退避

`DeviceState::switch_screen`は`take_placements()`でplacementをテーブルから外してidを返す。**pumpの掃引（`Vt::sweep_evictions`）では拾えない**ので、発生源で`VtSignal::WebviewEvicted`を出す必要がある。

### 未実装モードは黙って無視する

`set_private_modes`の`_ => {}`はそのままでよい。DECRQMを実装したときに0（未認識）で答えるのが正直な形になる。
