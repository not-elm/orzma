import { activateActivityEndpoint } from '../terminal/api';
import type { ActivityId, PaneId, WindowId } from './types';

export async function activateActivity(
  windowId: WindowId,
  paneId: PaneId,
  activityId: ActivityId,
): Promise<void> {
  try {
    const resp = await fetch(activateActivityEndpoint(windowId, paneId, activityId), {
      method: 'POST',
    });
    if (!resp.ok) {
      console.warn('activate activity failed', {
        windowId,
        paneId,
        activityId,
        status: resp.status,
      });
    }
  } catch (e) {
    console.warn('activate activity request errored', {
      windowId,
      paneId,
      activityId,
      error: e,
    });
  }
}
