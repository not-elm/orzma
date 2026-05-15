//! Paste event handling — CR normalization (xterm.js parity) plus bracketed
//! paste with intentional ESC sanitization (security hardening; NOT part of
//! xterm.js's behavior).

const ENC = new TextEncoder();

/** Encodes pasted text per current terminal modes.
 *
 * Always normalizes `\r?\n → \r` so multi-line paste runs each line in
 * raw-mode shells (xterm.js parity, `Clipboard.ts:13` in @xterm/xterm@5.5.0).
 *
 * Under bracketed-paste, wraps with `\e[200~ ... \e[201~` and replaces `\x1b`
 * with U+241B (visible ␛) inside the wrapper. The replacement is intentional
 * hardening to defeat `\e[201~` injection attacks from malicious clipboards;
 * xterm.js does not do this, but adding it is cheap and prevents a class of
 * paste-jacking.
 */
export function encodePaste(text: string, modes: ReadonlySet<string>): Uint8Array {
  const normalized = text.replace(/\r?\n/g, '\r');
  if (modes.has('bracketed-paste')) {
    // biome-ignore lint/suspicious/noControlCharactersInRegex: ESC sanitization intentionally targets the control character that would end the bracketed-paste wrapper.
    const sanitized = normalized.replace(//g, '␛');
    return ENC.encode(`\x1b[200~${sanitized}\x1b[201~`);
  }
  return ENC.encode(normalized);
}

/** Wires the textarea `paste` event. */
export function setupPaste(
  ta: HTMLTextAreaElement,
  modesRef: { current: ReadonlySet<string> },
  send: (bytes: Uint8Array) => void,
): () => void {
  const onPaste = (e: ClipboardEvent): void => {
    e.preventDefault();
    const text = e.clipboardData?.getData('text');
    if (!text) return;
    send(encodePaste(text, modesRef.current));
    // NOTE: defensive textarea reset prevents IME residue if the paste landed
    // during a stale composition state on a misbehaving browser.
    ta.value = '';
  };

  ta.addEventListener('paste', onPaste);
  return () => ta.removeEventListener('paste', onPaste);
}
