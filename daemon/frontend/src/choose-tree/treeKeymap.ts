import type { TreeAction } from './treeReducer';

export type ResolvedKey = TreeAction | { type: 'confirm' } | { type: 'cancel' };

/**
 * Maps a raw `KeyboardEvent` to a high-level picker action. Returns
 * `null` for any key the picker does not consume — modifier-augmented
 * keys, unrecognised characters, and IME composition events are all
 * filtered out at the caller; this function assumes those have already
 * been handled.
 */
export function keyToAction(e: KeyboardEvent): ResolvedKey | null {
  if (e.ctrlKey || e.altKey || e.metaKey) return null;
  switch (e.key) {
    case 'j':
    case 'ArrowDown':
      return { type: 'move', direction: 'down' };
    case 'k':
    case 'ArrowUp':
      return { type: 'move', direction: 'up' };
    case 'l':
    case 'ArrowRight':
      return { type: 'expand' };
    case 'h':
    case 'ArrowLeft':
      return { type: 'collapse' };
    case 'Enter':
      return { type: 'confirm' };
    case 'Escape':
      return { type: 'cancel' };
    default:
      return null;
  }
}
