# Ctrl+Backspace と Alt+Backspace を、ほとんどのターミナルと同じ形で送る

- Issue: #405
- 作成者: notelm
- ステータス: 承認済み
- 日付: 2026-10-22

## 問題

bash、zsh、fish、PowerShell(Emacs モード)では、Alt+Backspace(macOS では Option+Delete)が「前の単語を消す」キーであり、Claude Code も Option+Delete を同じ動作として挙げている。Windows の PowerShell と Claude Code では、Ctrl+Backspace も単語削除のキーである。

現在の orzma では、Ctrl+Backspace も Alt+Backspace も Backspace と同じ DEL をプログラムに送るので、どちらを押しても1文字しか消えない(コードから読み取ったもので、画面では確認していない)。

この問題はコード解析で見つかったもので、作成者が実際の利用で困った場面はまだない。作成者がこれを対象にしたのは、ほとんどのターミナルがこの形で送っているためである。

## 求める解決

- R1: プログラムが何も求めていないとき、Ctrl+Backspace を押すと、orzma は BS を送る。
- R2: プログラムが何も求めていないとき、Alt+Backspace を押すと、orzma は ESC DEL を送る。
- R3: プログラムが何も求めていないとき、Ctrl+Alt+Backspace を押すと、orzma は ESC BS を送る。

## 対象外

- Ctrl+Backspace・Alt+Backspace・Ctrl+Alt+Backspace 以外で、プログラムが何も求めていないときに送る形を変えること。たとえば Alt+Enter を ESC CR で送ることや、Alt+Escape・Alt+Tab に ESC を付けることは扱わない。

## 検討した代替案

- なし

## 未決事項

- なし

## Step 2 への引き継ぎメモ
