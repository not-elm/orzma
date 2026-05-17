import { breakActivityToPane } from '../layout/breakActivityToPane';
import { closeActivity } from '../layout/closeActivity';
import { closePane } from '../layout/closePane';
import { createWindow } from '../layout/createWindow';
import { cycleActivity } from '../layout/cycleActivity';
import { focusPane } from '../layout/focusPane';
import { newTerminalActivity } from '../layout/newTerminalActivity';
import { resizePane } from '../layout/resizePane';
import { splitPane } from '../layout/splitPane';
import type { ActivityId, PaneId, WindowId } from '../layout/types';
import type { SessionView } from '../statusbar/types';
import { windowSelect } from '../statusbar/windowSelect';
import type { Action } from './wire';

export interface ShortcutContext {
  activeWindow: () => WindowId | null;
  activePane: () => PaneId | null;
  activeActivity: () => ActivityId | null;
  /** Latest session snapshot or `null` while loading. */
  activeSession: () => SessionView | null;
  /** Opens the rename-window prompt for the active window. */
  openRenameWindow: () => void;
  /** Opens the cross-session window picker (tmux choose-tree). */
  openChooseTree: () => void;
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
    case 'rename-window':
      return () => ctx.openRenameWindow();
    case 'choose-tree':
      return () => ctx.openChooseTree();
    case 'new-window':
      return () => {
        const view = ctx.activeSession();
        if (view) void createWindow(view.id);
      };
    case 'split-pane': {
      const orientation = action.direction;
      return () => withActivePane(ctx, (w, p) => splitPane(w, p, orientation));
    }
    case 'break-activity-to-pane': {
      const orientation = action.direction;
      return () =>
        withActivePaneActivity(ctx, (w, p, a) => breakActivityToPane(w, p, a, orientation));
    }
    case 'new-terminal-activity':
      return () => withActivePane(ctx, newTerminalActivity);
    case 'close-activity':
      return () => withActivePaneActivity(ctx, closeActivity);
    case 'focus-activity': {
      const direction = action.offset;
      return () => withActivePane(ctx, (w, p) => cycleActivity(w, p, direction));
    }
    case 'focus-window': {
      const offset = action.offset;
      return () => {
        const view = ctx.activeSession();
        if (!view || view.windows.length < 2) return;
        const active = view.active_window;
        const pos = view.windows.findIndex((w) => w.id === active);
        if (pos < 0) return;
        const delta = offset === 'next' ? 1 : -1;
        const next = view.windows[(pos + delta + view.windows.length) % view.windows.length];
        if (next) void windowSelect(next.id);
      };
    }
    case 'focus-window-number': {
      const index = action.index;
      return () => {
        const view = ctx.activeSession();
        if (!view) return;
        const target = view.windows.find((w) => w.index === index);
        if (!target) return;
        void windowSelect(target.id);
      };
    }
    case 'focus-pane': {
      const direction = action.direction;
      return () => withActiveWindow(ctx, (w) => focusPane(w, direction));
    }
    case 'resize-pane': {
      const direction = action.direction;
      return () => withActivePane(ctx, (w, p) => resizePane(w, p, direction));
    }
    default:
      console.warn('actionToHandler: unsupported action', action);
      return null;
  }
}

function withActiveWindow(ctx: ShortcutContext, run: (w: WindowId) => void | Promise<void>): void {
  const w = ctx.activeWindow();
  if (w) void run(w);
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
