import { closeActivityEndpoint } from '../terminal/api';
import type { ActivityId, PaneId, WindowId } from './types';

export async function closeActivity(
  windowId: WindowId,
  paneId: PaneId,
  activityId: ActivityId,
): Promise<void> {
  try {
    const resp = await fetch(closeActivityEndpoint(windowId, paneId, activityId), {
      method: 'DELETE',
    });
    if (!resp.ok) {
      console.warn('close activity failed', {
        windowId,
        paneId,
        activityId,
        status: resp.status,
      });
    }
  } catch (e) {
    console.warn('close activity request errored', {
      windowId,
      paneId,
      activityId,
      error: e,
    });
  }
}
