# @orzma/web

A typed client for `window.orzma`, the bridge that orzma injects into webview
pages. A page uses it to call methods in the program that registered it, to
receive the program's events, and to send events back.

```sh
npm install @orzma/web
```

```ts
import { isOrzmaAvailable, orzma } from '@orzma/web';

if (isOrzmaAvailable()) {
  orzma.on<number>('tick', (n) => {
    document.title = `tick ${n}`;
  });
  orzma.emit('ready');
}
```

See [Building Webview Apps](https://not-elm.github.io/orzma/building-webview-apps.html#the-page-side)
for how the page and the program fit together, and the
[Webview Protocol](https://not-elm.github.io/orzma/protocol-reference.html#the-windoworzma-bridge)
for the bridge's full behavior.
