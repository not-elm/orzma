import type { ShortcutContext } from './actionDispatch';

export function makeShortcutContext(overrides?: Partial<ShortcutContext>): ShortcutContext {
  return {
    activeWindow: () => null,
    activePane: () => null,
    activeActivity: () => null,
    activeSession: () => null,
    openRenameWindow: () => {},
    ...overrides,
  };
}
