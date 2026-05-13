import { closePane } from '../layout/closePane';
import { splitPane } from '../layout/splitPane';
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
  switch (action.type) {
    case 'close-pane':
      return () => withActivePane(ctx, closePane);
    case 'split-pane': {
      const orientation = action.direction;
      return () => withActivePane(ctx, (w, p) => splitPane(w, p, orientation));
    }
    default:
      console.warn('actionToHandler: unsupported action', action);
      return null;
  }
}

function withActivePane(
  ctx: ShortcutContext,
  run: (w: WindowId, p: PaneId) => void | Promise<void>,
): void {
  const w = ctx.activeWindow();
  const p = ctx.activePane();
  if (w && p) void run(w, p);
}
