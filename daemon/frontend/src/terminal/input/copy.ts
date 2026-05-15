//! Native clipboard copy: hook the `copy` event on the focused textarea,
//! read document.getSelection(), trim per-line trailing whitespace, convert
//! NBSP (used by Row.tsx for underline cells per R4) back to space, and
//! write the result to clipboardData. Multi-row line break is \n (Tauri
//! limited to local UI; no \r\n parity needed).

/** Pure formatting helper — exported for unit testing. */
export function formatSelectionText(raw: string): string {
  if (!raw) return '';
  return raw
    .split('\n')
    .map((line) => line.replace(/ /g, ' ').replace(/[ \t]+$/, ''))
    .join('\n');
}

/** Attaches a `copy` listener to `textarea` that intercepts native browser
 *  copy and writes the formatted selection text. Returns a cleanup. */
export function setupCopy(textarea: HTMLTextAreaElement): () => void {
  const onCopy = (e: Event): void => {
    const ce = e as ClipboardEvent;
    const sel = document.getSelection();
    if (!sel || sel.rangeCount === 0) return;
    const raw = sel.toString();
    if (!raw) return;
    e.preventDefault();
    ce.clipboardData?.setData('text/plain', formatSelectionText(raw));
  };
  textarea.addEventListener('copy', onCopy);
  return () => textarea.removeEventListener('copy', onCopy);
}
