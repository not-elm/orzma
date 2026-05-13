import { closePane } from '../layout/closePane';
import type { PaneId, WindowId } from '../layout/types';
import type { Action } from './wire';

export interface ShortcutContext {
  activeWindow: () => WindowId | null;
  activePane: () => PaneId | null;
}

/**
 * Map a parsed `Action` to a side-effect handler that reads the active
 * pane/window from `ctx` at fire time. Returns `null` for actions the
 * frontend has not implemented yet (`console.warn` once at construction).
 */
export function actionToHandler(action: Action, ctx: ShortcutContext): (() => void) | null {
  if (action.type === 'close-pane') {
    return () => {
      const w = ctx.activeWindow();
      const p = ctx.activePane();
      if (w && p) {
        void closePane(w, p);
      }
    };
  }
  console.warn('actionToHandler: unsupported action', action);
  return null;
}
