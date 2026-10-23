# プログラムが求めたときに、キーの区別・リピート・離したことを届ける

- Issue: #405
- 作成者: notelm
- ステータス: 承認済み
- 日付: 2026-10-22

## 問題

orzma の中で Claude Code などの AI CLI を使う人が Shift+Enter で改行しようとすると、改行されずに入力が送信されてしまう。

この問題は作成者の実体験から出たものではない。作成者の言葉では「CLaudeCodeのコード解析で判明した問題」である。

現在の orzma では、Shift+Enter、Ctrl+Enter、Alt+Enter はどれも Enter と同じものとしてプログラムに届く。そのため Claude Code の中で改行に使えるのは、Ctrl+J と `\`+Enter だけである(調査から読み取ったもので、画面では確認していない)。Vim でも、Shift+Enter は Enter と区別されない。

Shift+Enter のほかにも、調べた 12 のターミナルのうち 8 つは、求めたプログラムに、修飾キー付きのキーの区別、キーのリピートと離したこと、Shift で入る文字や英語配列での位置、文字のキーを含むすべてのキーの区別を届けている。orzma では、どのプログラムもこれらを受け取れない。たとえば Helix は、これらを受け取れないと、キーの区別そのものを使わない(ソースから読み取ったもので、画面では確認していない)。作成者がこれらも対象にしたのは、ほとんどのターミナルが対応しているためである。

## 求める解決

- R1: orzma の中で Claude Code を使っているとき、Shift+Enter を押すと、入力は送信されずに改行が入る。
- R2: Shift+Enter と Enter を区別して受け取りたいと orzma に求めてきたプログラムには、それがどのプログラムであっても、Shift+Enter が Enter と区別できる形で届く。
- R3: R2 は、Vim のように modifyOtherKeys で区別を求めてくるプログラムにも成り立つ。
- R4: 区別を求めたプログラムが元に戻さずに終了したあとでも、シェルで `reset` と打って Enter を押せば、キーはそのプログラムが区別を求める前と同じ形で届くようになる。ただし、そのプログラムが kitty keyboard protocol の flag 8 か modifyOtherKeys のレベル 3 を求めていた場合は除く。
- R5: kitty keyboard protocol の flag 1(エスケープコードの曖昧さをなくす)を求めたプログラムには、修飾キー付きの Enter・Tab・Backspace、修飾キーの有無にかかわらず Escape、Ctrl・Alt・Ctrl+Alt・Shift+Alt を押しながらのキー、テンキーのキーが、仕様の定める形でほかのキーと区別できるように届き、修飾キーなしの Enter・Tab・Backspace は今と同じ形で届く。
- R6: R5 のプログラムには、Ctrl+C も中断の合図(SIGINT)にならずにキーとして届く。
- R7: modifyOtherKeys で区別を求めたプログラムには、Shift+Enter のほかの修飾キー付きのキーも、求めたレベルについて xterm のマニュアルが定める形で、区別できるように届く。
- R8: modifyOtherKeys を求めたプログラムに届くテンキーのキーは、今と同じ形のままである。
- R9: kitty keyboard protocol と modifyOtherKeys の両方を求めたプログラムには、kitty keyboard protocol のフラグが1つでも有効なあいだは kitty keyboard protocol の形で、すべて外れたあとは modifyOtherKeys の形で、キーが届く。
- R10: flag 2(イベントの種類を伝える)を求めたプログラムには、仕様が伝えると定めるキーについて、キーを押し続けたときのリピートと、キーを離したことが届く。
- R11: キーを押してから離すまでのあいだに、別のペインに移る、同じペインの web ページがキーボードを受け取る、vi モードに入る、orzma のウィンドウが OS のフォーカスを失う、のいずれかが起きると、押したことを受け取ったプログラムには、その時点で押されていて、そのとき有効なフラグで離したことを伝えるキーすべての「離した」が届く。
- R12: R11 のあと、キーを押したままキーボードがそのプログラムに戻ると(元のペインに戻る、web ページがキーボードを返す、vi モードを抜ける、orzma のウィンドウが OS のフォーカスを取り戻す)、そのプログラムには、まだ押されているキーの「押した」がもう一度届き、そのキーを離したときに「離した」が届く。
- R13: kitty keyboard protocol や modifyOtherKeys を求めたプログラムがあっても、orzma のショートカットに割り当てられたキーは今どおり orzma が使い、ショートカットを実行したキーは、修飾キーを除いて、押したこと・リピート・離したことのどれもプログラムに届かない。
- R14: flag 8(すべてのキーをエスケープコードで送る)を求めたプログラムには、Alt・Ctrl・Shift を単独で押したことが、それをショートカットやリーダーキーに使った場合でも届き、flag 2 も求めていれば離したことも届く。
- R15: flag 4(別のキーを伝える)を求めたプログラムには、Shift を押したときに入る文字と、英語配列で同じ位置にあるキーが、仕様の定める形でキーといっしょに届く。
- R16: flag 8 を求めたプログラムには、文字のキー、修飾キーなしの Enter・Tab・Backspace、修飾キーを単独で押したことも、仕様の定める形で区別できるように届く。
- R17: flag 8 とともに flag 16(入力された文字を添える)を求めたプログラムには、キーが入力する文字が、仕様の定める形でキーといっしょに届く。
- R18: flag 8 や flag 16 を求めたプログラムにも、IME で確定した文字は、今と同じく文字のまま届く。
- R19: kitty keyboard protocol を求めたプログラムにも、orzma のショートカットに割り当てられていない Cmd(macOS)・Super(Windows・Linux)付きのキーと、Cmd・Super を単独で押したことは届かない。

## 対象外

- orzma の中で tmux を起動し、その中で Claude Code などを使う場合に、Shift+Enter で改行できること。
- プログラムが何も求めていないときに送る形を変えること。Ctrl+Backspace と Alt+Backspace は別の要件書で扱う。
- kitty keyboard protocol の flag 8 か modifyOtherKeys のレベル 3 が残ったペインを、orzma の操作で元に戻せること。#418 で扱う。
- Windows で R1 を満たすこと。Windows では、Claude Code が kitty keyboard protocol に対応しているかを問い合わせると、orzma の返事より先に ConPTY の返事が届くため、Claude Code は orzma を非対応と判定する(計測で確認した)。
- Windows で R4 を満たすこと。

## 検討した代替案

- フォーカスが移ったとき、Ghostty と同じく、最後に押したキーだけを離したことにする(提案: 人間) — 押していたキーすべてを離したことにするほうを、作成者が選んだため。
- 押したままペインに戻ったとき、何も送らず、そのあと離したときの「離した」も送らない(提案: AI) — 「押した」をもう一度送るほうを、作成者が選んだため。
- IME で確定した文字を、仕様どおりキー番号 0 の「文字だけの入力」として送る(提案: AI) — kitty と Ghostty と同じく今のままの形で送るほうを、作成者が選んだため。
- kitty keyboard protocol を求めたプログラムに、Cmd・Super 付きのキーを kitty や Ghostty のように届ける(提案: AI) — 届けると、Kakoune で Cmd+D がただの `d` として届いて選択範囲が消えるなどの問題があり、作成者が #404 のとおり捨てるほうを選んだため。
- flag 8 と modifyOtherKeys のレベル 3 を対象から外し、R4 を元のまま保つ(提案: AI) — ほかのターミナルに合わせて R4 の適用範囲を狭めるほうを、作成者が選んだため。
- orzma 自身のリセット操作を今回用意する(提案: AI) — 優先度の低い別の Issue(#418)として扱うことを、作成者が選んだため。
- R4 を、`reset` コマンドのない PowerShell と cmd でだけ対象外にする(提案: AI) — Windows 全体で対象外にするほうを、作成者が選んだため。

## 未決事項

- なし

## Step 2 への引き継ぎメモ

- 「Keep the flag stacks in the VT device (`orzma_vt`) and pass the active flags to the key encoder, as DECCKM is passed today.」(Issue #405)
