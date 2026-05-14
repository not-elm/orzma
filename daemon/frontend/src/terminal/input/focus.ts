//! DECSET 1004 focus event reporting on the textarea.

const ENC = new TextEncoder();
const FOCUS_IN = ENC.encode('\x1b[I');
const FOCUS_OUT = ENC.encode('\x1b[O');

/** Sends `\e[I` on focus and `\e[O` on blur — but only when `focus-events`
 *  is in `modesRef.current` at event time. */
export function setupFocusEvents(
  ta: HTMLTextAreaElement,
  modesRef: { current: ReadonlySet<string> },
  send: (bytes: Uint8Array) => void,
): () => void {
  const onFocus = (): void => {
    if (modesRef.current.has('focus-events')) send(FOCUS_IN);
  };
  const onBlur = (): void => {
    if (modesRef.current.has('focus-events')) send(FOCUS_OUT);
  };
  ta.addEventListener('focus', onFocus);
  ta.addEventListener('blur', onBlur);
  return () => {
    ta.removeEventListener('focus', onFocus);
    ta.removeEventListener('blur', onBlur);
  };
}
