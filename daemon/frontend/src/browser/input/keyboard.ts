import type { BrowserClientMsg, KeyKind } from '../protocol/wire';
import { modBits } from './mouse';

/**
 * Attach keydown/keyup listeners to the hidden textarea backing the
 * browser surface. `globalKeyDispatcher` runs in the capture phase and
 * intercepts the `Ctrl+B` prefix before keys reach here, so we send
 * everything else through to CDP as-is.
 */
export function attachKeyboard(
  ta: HTMLTextAreaElement,
  send: (m: BrowserClientMsg) => void,
): () => void {
  const sendKey = (kind: KeyKind) => (e: KeyboardEvent) => {
    send({
      kind: 'key',
      key_kind: kind,
      code: e.code,
      key: e.key,
      text: kind === 'down' && e.key.length === 1 ? e.key : null,
      modifiers: modBits(e),
    });
  };
  const onDown = sendKey('down');
  const onUp = sendKey('up');
  ta.addEventListener('keydown', onDown);
  ta.addEventListener('keyup', onUp);
  return () => {
    ta.removeEventListener('keydown', onDown);
    ta.removeEventListener('keyup', onUp);
  };
}
