import type { BrowserClientMsg } from '../protocol/wire';

/**
 * Wire IME composition events on the hidden textarea. `compositionupdate`
 * forwards the in-progress preedit; `compositionend` commits the final text.
 *
 * Chromium renders the preedit inside the page's real input field, so it
 * appears in the screencast with no frontend overlay.
 */
export function attachComposition(
  ta: HTMLTextAreaElement,
  send: (m: BrowserClientMsg) => void,
): () => void {
  const onUpdate = (e: CompositionEvent) => {
    const target = e.target as HTMLTextAreaElement;
    send({
      kind: 'ime_composition',
      text: e.data ?? '',
      selection_start: target.selectionStart,
      selection_end: target.selectionEnd,
    });
  };
  const onEnd = (e: CompositionEvent) => {
    send({ kind: 'ime_commit', text: e.data ?? '' });
  };
  ta.addEventListener('compositionupdate', onUpdate);
  ta.addEventListener('compositionend', onEnd);
  return () => {
    ta.removeEventListener('compositionupdate', onUpdate);
    ta.removeEventListener('compositionend', onEnd);
  };
}
