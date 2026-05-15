import type { BrowserClientMsg } from '../protocol/wire';

/**
 * Read text from the OS clipboard and forward it to the page via
 * `BrowserClientMsg::Paste`. Silently ignores permission denials.
 */
export async function pasteFromOs(send: (m: BrowserClientMsg) => void): Promise<void> {
  try {
    const text = await navigator.clipboard.readText();
    send({ kind: 'paste', text });
  } catch {
    // NOTE: clipboard read may be denied (permissions, focus) — ignore.
  }
}

/**
 * Ask the daemon for the page's current selection. The daemon will reply
 * with `BrowserServerMsg::ClipboardWrite`, which the WS hook writes into
 * the OS clipboard via `navigator.clipboard.writeText`.
 */
export function requestCopy(send: (m: BrowserClientMsg) => void): void {
  send({ kind: 'copy_request' });
}
