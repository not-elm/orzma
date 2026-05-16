import { cycleActivityEndpoint } from '../terminal/api';
import type { PaneId, WindowId } from './types';

export async function cycleActivity(
  windowId: WindowId,
  paneId: PaneId,
  direction: 'next' | 'prev',
): Promise<void> {
  try {
    const resp = await fetch(cycleActivityEndpoint(windowId, paneId), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ direction }),
    });
    if (!resp.ok) {
      console.warn('cycle activity failed', {
        windowId,
        paneId,
        direction,
        status: resp.status,
      });
    }
  } catch (e) {
    console.warn('cycle activity request errored', {
      windowId,
      paneId,
      direction,
      error: e,
    });
  }
}
