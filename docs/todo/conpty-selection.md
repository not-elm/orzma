# ConPTY の選択

portable-pty 0.9.0 は、DLL の検索順（PATH を含む）で見つけた `conpty.dll` を OS 内蔵の ConPTY より優先して読み込む。そのため、どの ConPTY が使われるかが環境で変わる。

- PATH に WezTerm がある環境では、WezTerm 同梱の OpenConsole（2023 年版、`--resizeQuirk` 付き）が使われる。リサイズ後は再描画せず、CUP を 1 つ送るだけ。
- それ以外では OS 内蔵の conhost が使われる（Windows 11 26200 では quirk を無視し、リサイズのたびに画面全体を再描画する）。
- PATH 経由の DLL 読み込みでもある。

決めること:

- OS 内蔵の ConPTY に固定する（portable-pty へのパッチか、独自の ConPTY ラッパーが必要）。
- `conpty.dll` と `OpenConsole.exe` を orzma に同梱して挙動を 1 つに固定する。先例として、node-pty は `Microsoft.Windows.Console.ConPTY` の `conpty.dll` と `OpenConsole.exe` を同梱して読み込む（`useConptyDll`）。MSI とビルドスクリプトの変更が必要。

未確認: Windows 10 の OS 内蔵 ConPTY でのリサイズの挙動（折り返しを自動折り返しで送るか）。
