# F キー・修飾キー付きのナビゲーションキー・Ctrl+記号を xterm と同じバイト列で送る

- Issue: #404
- 作成者: notelm
- ステータス: 承認済み
- 日付: 2026-10-21

## 問題

orzma の中で、`TERM=xterm-256color` を前提にキー入力を読むプログラムを使うユーザーが、F キー、修飾キー付きの矢印などのキー、Ctrl+記号を使えない。htop や mc の F キーメニュー、シェルの単語単位の移動、vim や tmux での修飾キー付きの矢印、`Ctrl+Space`(マークの設定)、`Ctrl+\`(SIGQUIT)、`Ctrl+_`(readline の取り消し)が働かない。

この問題はコード解析で見つかったもので、作成者が実際の利用で困った場面はまだない。

現在は次のようになっている。

- F1〜F12 を押しても、プログラムには何も送られない。
- 矢印・Home・End・Insert・Delete・PageUp・PageDown に修飾キーを付けても、修飾キーなしと同じバイト列が送られる。たとえば `Ctrl+←` は `←` と同じになる。
- Ctrl で制御文字になるのは英字だけで、`Ctrl+Space`、`Ctrl+[`、`Ctrl+\`、`Ctrl+/` などでは記号がそのまま送られる。`Ctrl+2`〜`Ctrl+8` でも数字がそのまま送られる。
- `Ctrl+Alt` と英字を押すと、制御文字だけが送られ、前に `ESC` が付かない。

## 求める解決

以下の m は、1 に、押している修飾キーの値(Shift が 1、Alt が 2、Ctrl が 4)を足した数である。

- R1: F1〜F4 を修飾キーなしで押すと、orzma はそれぞれ `SS3 P`、`SS3 Q`、`SS3 R`、`SS3 S` をプログラムに送る。
- R2: F5〜F12 を修飾キーなしで押すと、orzma はそれぞれ `CSI 15 ~`、`CSI 17 ~`、`CSI 18 ~`、`CSI 19 ~`、`CSI 20 ~`、`CSI 21 ~`、`CSI 23 ~`、`CSI 24 ~` を送る。
- R3: Shift・Alt・Ctrl のいずれかを押しながら F1〜F4 を押すと、orzma はそれぞれ `CSI 1 ; m P`、`CSI 1 ; m Q`、`CSI 1 ; m R`、`CSI 1 ; m S` を送る。
- R4: Shift・Alt・Ctrl のいずれかを押しながら F5〜F12 を押すと、orzma は R2 の数字を n として `CSI n ; m ~` を送る。
- R5: Shift・Alt・Ctrl のいずれかを押しながら ↑・↓・→・←・Home・End を押すと、orzma はアプリケーションカーソルキーモード(DECCKM)にかかわらず、それぞれ `CSI 1 ; m A`、`CSI 1 ; m B`、`CSI 1 ; m C`、`CSI 1 ; m D`、`CSI 1 ; m H`、`CSI 1 ; m F` を送る。
- R6: Shift・Alt・Ctrl のいずれかを押しながら Insert・Delete・PageUp・PageDown を押すと、orzma はそれぞれ `CSI 2 ; m ~`、`CSI 3 ; m ~`、`CSI 5 ; m ~`、`CSI 6 ; m ~` を送る。
- R7: Ctrl を押しながら `Space` か `@` を押すと NUL、`[` を押すと ESC、`\` を押すと FS、`]` を押すと GS、`^` か `~` を押すと RS、`_` か `/` を押すと US、`?` を押すと DEL を、orzma は送る。
- R8: Ctrl を押しながら `2`〜`8` を押すと、orzma はそれぞれ NUL、ESC、FS、GS、RS、US、DEL を送る。
- R9: Ctrl と Alt を押しながら英字か R7・R8 のキーを押すと、orzma は Ctrl だけのときに送る制御文字の前に `ESC` を付けて送る。

## 対象外

- ショートカットに割り当てられていない Cmd(macOS)や Super(Windows・Linux)付きのキーをプログラムに届けること。今どおり捨てるので、m に Meta の値は加わらない。
- 修飾キー付きの Enter・Tab・Backspace・Escape を、修飾キーなしと区別して送ること。区別は kitty keyboard protocol の Issue で扱う。
- キー割り当てで F キーを使えるようにすること。別の Issue で扱う。
- F13 以上のキー。今どおり何も送らない。

## 検討した代替案

- ショートカットに割り当てられていない Cmd・Super 付きのキーを、Meta(値 8)としてプログラムに届ける(提案: 人間、Issue #404 の修飾キーの値に Meta 8 が含まれていた) — xterm でも Meta はふつう Alt キーを指し、修飾値 9〜16 は既定では使われないこと、受け取るプログラムによって解釈が違うこと、Cmd の押し間違いがシェルの操作として届くおそれがあることがわかり、作成者が捨てるほうを選んだため。
- Cmd・Super 付きのキーは既定では捨て、望む人が設定で届くようにできるようにする(提案: AI) — 作成者が捨てるほうを選んだため。
- `Ctrl+?` で、xterm 本体と同じく `?` をそのまま送る(提案: AI) — 作成者が、多くのターミナルと同じ DEL を選んだため。
- F13 以上のキーにもバイト列を割り当てる(提案: 人間、Issue #404 の未決事項) — 作成者が今回は扱わないとしたため。

## 未決事項

- macOS の既定の設定で OS が先に受け取るキー(`Ctrl+←`・`Ctrl+→`・`Ctrl+↑`・`Ctrl+Space`)について、orzma の側で何かすることを求めるか。(承認を妨げる: いいえ)

## Step 2 への引き継ぎメモ

- 「Add the function keys to `TerminalKey` and map Bevy's `Key::F1`–`Key::F12`.」(Issue #404)
- 「Pass the modifiers into the cursor and editing key encoders and emit the `CSI … ; m` forms when any modifier is held. With a modifier, xterm sends `CSI` even in application cursor mode.」(Issue #404)
- 「Turn `ctrl_letter_byte` into a table covering the symbols above.」(Issue #404)
- 「Update `terminal-compatibility.md` and the test that pins `F1`.」(Issue #404)
