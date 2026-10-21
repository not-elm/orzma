# Shift+Enter を Enter と区別してプログラムに届ける

- Issue: #405
- 作成者: notelm
- ステータス: 承認済み
- 日付: 2026-10-21

## 問題

orzma の中で Claude Code などの AI CLI を使う人が Shift+Enter で改行しようとすると、改行されずに入力が送信されてしまう。

この問題は作成者の実体験から出たものではない。作成者の言葉では「CLaudeCodeのコード解析で判明した問題」である。

現在の orzma では、Shift+Enter、Ctrl+Enter、Alt+Enter はどれも Enter と同じものとしてプログラムに届く。そのため Claude Code の中で改行に使えるのは、Ctrl+J と `\`+Enter だけである(調査から読み取ったもので、画面では確認していない)。Vim でも、Shift+Enter は Enter と区別されない。

## 求める解決

- R1: orzma の中で Claude Code を使っているとき、Shift+Enter を押すと、入力は送信されずに改行が入る。
- R2: Shift+Enter と Enter を区別して受け取りたいと orzma に求めてきたプログラムには、それがどのプログラムであっても、Shift+Enter が Enter と区別できる形で届く。
- R3: R2 は、Vim のように modifyOtherKeys で区別を求めてくるプログラムにも成り立つ。
- R4: 区別を求めたプログラムが元に戻さずに終了したあとでも、シェルで `reset` と打って Enter を押せば、キーはそのプログラムが区別を求める前と同じ形で届くようになる。

## 制約

- なし

## 対象外

- orzma の中で tmux を起動し、その中で Claude Code などを使う場合に、Shift+Enter で改行できること。
- Shift+Enter 以外のキー(Ctrl+Enter、Alt+Enter、Tab と Ctrl+I、Escape と Ctrl+[、Backspace と Ctrl+H など)を区別して届けること。これらが今のままであることも求めない。

## 検討した代替案

- なし

## 未決事項

- キーを離したことや、キーを押し続けたときのリピートを、プログラムに伝えるかどうか。(承認を妨げる: いいえ)

## Step 2 への引き継ぎメモ
