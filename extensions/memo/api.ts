import { readFile } from 'node:fs/promises';

/**
 * memo's host API. The single `fs` namespace exposes file reads to the mounted
 * webview; the view's `capabilities = ["fs"]` grant (in `ozmux.toml`) is what
 * lets `window.fs.read(...)` reach this code. Erasable TS only (Node native
 * type-stripping): no `enum` / parameter-properties / `namespace`.
 */
export default {
  fs: {
    read: (path: string): Promise<Uint8Array> => readFile(path),
  },
};
