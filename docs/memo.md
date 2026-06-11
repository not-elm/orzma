## Phase1
現在Extensionごとにホストプロセスを起動していますが、これを廃止し、アプリ上にただ１つのNodeJsプロセスを起動するようにしたい。
これに伴い、Bootstrap,拡張コマンド、Handlerは廃止する。
WebviewからOSへのリソースにアクセスできるようにエンドユーザーからホストプロセスにAPIを拡張できるようにする。
たとえばNodejsのfsを使うようにするためのAPIの疑似コードは以下になる。
```ts
// api.ts
export default {
  fs: {
    read: (path: string) => fs.readSync(path)
  }  
}
// Webview内
const buf = await window.fs.read("PATH");
```

## Phase2
WebviewはOSCレンダリングで表示する。
Kittyのようにターミナル上にインラインでWebviewのテクスチャを埋め込めるようにする。
現状はAlternateScreenを用意してそこにレンダリングするだけでいい。
たとえば以下のようなOSCをイメージしている
```
1049h:

  alternate screenに入る

OSC webview.mount:

  alt screen内にWebViewを配置

1049l:

  WebViewを自動unmount
```
現状の"@memo"は廃止されるため、大体となるアプローチとしてOSCを使ったWebviewのマウントに切り替える

## Phase3

Tmux -CCサポート
現状はアプリ内でペインのレイアウトを管理しているが、Tmuxを利用する方法に置き換える。
通常は単一のターミナルエミュレータとして起動するが、ショートカットでTmuxのセッション一覧を表示できるようにし、指定されたセッションをTmux -CCモードでアタッチする方式。

