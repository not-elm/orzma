import { resizePaneEndpoint } from '../terminal/api';
import type { PaneId, WindowId } from './types';

export type ResizeDirection = 'left' | 'right' | 'up' | 'down';

export async function resizePane(
  windowId: WindowId,
  paneId: PaneId,
  direction: ResizeDirection,
  amount = 1,
): Promise<void> {
  try {
    const resp = await fetch(resizePaneEndpoint(windowId, paneId), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ direction, amount }),
    });
    if (resp.status === 409) {
      // NOTE: 409 = WINDOW_NOT_MEASURED (page-load race) or PANE_NOT_IN_WINDOW (stale active pane). Drop quietly — user retries on next keypress.
      console.debug('resizePane: 409', await resp.text());
      return;
    }
    if (!resp.ok) {
      console.warn('resize pane failed', {
        windowId,
        paneId,
        direction,
        amount,
        status: resp.status,
      });
    }
  } catch (e) {
    console.warn('resize pane request errored', { windowId, paneId, direction, amount, error: e });
  }
}
