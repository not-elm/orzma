import { addActivityEndpoint } from '../terminal/api';
import { activateActivity } from './activateActivity';
import type { PaneId, WindowId } from './types';

export async function newTerminalActivity(windowId: WindowId, paneId: PaneId): Promise<void> {
  const activityId = crypto.randomUUID();
  try {
    const resp = await fetch(addActivityEndpoint(windowId, paneId), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        activity: { activity_id: activityId, kind: { type: 'terminal' } },
      }),
    });
    if (!resp.ok) {
      console.warn('new terminal activity failed', {
        windowId,
        paneId,
        status: resp.status,
      });
      return;
    }
  } catch (e) {
    console.warn('new terminal activity request errored', {
      windowId,
      paneId,
      error: e,
    });
    return;
  }

  await activateActivity(windowId, paneId, activityId);
}
