import { breakActivityToPaneEndpoint } from '../terminal/api';
import type { ActivityId, PaneId, SplitOrientation, WindowId } from './types';

export async function breakActivityToPane(
  windowId: WindowId,
  paneId: PaneId,
  activityId: ActivityId,
  orientation: SplitOrientation,
): Promise<void> {
  try {
    const resp = await fetch(breakActivityToPaneEndpoint(windowId, paneId, activityId), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ orientation }),
    });
    if (!resp.ok) {
      console.warn('break activity to pane failed', {
        windowId,
        paneId,
        activityId,
        orientation,
        status: resp.status,
      });
    }
  } catch (e) {
    console.warn('break activity to pane request errored', {
      windowId,
      paneId,
      activityId,
      orientation,
      error: e,
    });
  }
}
