import { focusPaneEndpoint } from '../terminal/api';
import type { PaneDirection, WindowId } from './types';

export async function focusPane(windowId: WindowId, direction: PaneDirection): Promise<void> {
  try {
    const resp = await fetch(focusPaneEndpoint(windowId), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ direction }),
    });
    if (!resp.ok) {
      console.warn('focus pane failed', { windowId, direction, status: resp.status });
    }
  } catch (e) {
    console.warn('focus pane request errored', { windowId, direction, error: e });
  }
}
