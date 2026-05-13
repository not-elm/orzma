import { splitPaneEndpoint } from '../terminal/api';
import type { PaneId, SplitOrientation, WindowId } from './types';

export async function splitPane(
  windowId: WindowId,
  paneId: PaneId,
  orientation: SplitOrientation,
): Promise<void> {
  try {
    const resp = await fetch(splitPaneEndpoint(windowId, paneId), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ orientation }),
    });
    if (!resp.ok) {
      console.warn('split pane failed', { windowId, paneId, orientation, status: resp.status });
    }
  } catch (e) {
    console.warn('split pane request errored', { windowId, paneId, orientation, error: e });
  }
}
