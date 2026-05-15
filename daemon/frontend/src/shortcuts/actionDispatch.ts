import { closeActivity } from '../layout/closeActivity';
import { closePane } from '../layout/closePane';
import { newTerminalActivity } from '../layout/newTerminalActivity';
import { splitPane } from '../layout/splitPane';
import type { ActivityId, PaneId, WindowId } from '../layout/types';
import type { Action } from './wire';

export interface ShortcutContext {
  activeWindow: () => WindowId | null;
  activePane: () => PaneId | null;
  activeActivity: () => ActivityId | null;
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
    case 'new-terminal-activity':
      return () => withActivePane(ctx, newTerminalActivity);
    case 'close-activity':
      return () => withActivePaneActivity(ctx, closeActivity);
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

function withActivePaneActivity(
  ctx: ShortcutContext,
  run: (w: WindowId, p: PaneId, a: ActivityId) => void | Promise<void>,
): void {
  const w = ctx.activeWindow();
  const p = ctx.activePane();
  const a = ctx.activeActivity();
  if (w && p && a) void run(w, p, a);
}
