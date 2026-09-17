ペイン起動時のパス

- [x] Paneを分割する際に、分割時にフォーカスされていたPaneのCWDを継承する（macOS / Linux）。分割元のフォアグラウンドのプロセス、読めなければシェル、それも読めなければ最後に OSC 7 で報告されたディレクトリ、spawn 時のディレクトリの順に使う。
- [ ] Windows でも継承する。シェルが報告するディレクトリ（OSC 7 / OSC 9;9）を先に使い、PEB（`NtQueryInformationProcess` + `ReadProcessMemory`）はシェル本体だけをフォールバックで読む。pwsh の `Set-Location` はプロセスの cwd を変えないので、PEB を先に読むと起動時のディレクトリが返る。前提として、OSC 7 パーサが `file:///C:/Users/x` を `/C:/Users/x` にし、ホスト部を捨てる（UNC にできない）問題を直し、OSC 9;9 に対応する。
- [ ] macOS / Linux でも、`cd` がプロセスの cwd を変えないシェル（pwsh など）では OSC 7 を優先する。kitty 方式では、シェル自身がフォアグラウンドにいて OSC 7 もそのとき届いた場合だけ報告値を使う。
- [ ] OSC 7 パーサがホスト部を捨てるので、リモートシェル（ssh 先）の OSC 7 が分割元の報告ディレクトリを上書きする。
