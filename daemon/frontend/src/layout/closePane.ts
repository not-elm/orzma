import { closePaneEndpoint } from '../terminal/api';
import type { PaneId, WindowId } from './types';

export async function closePane(windowId: WindowId, paneId: PaneId): Promise<void> {
  try {
    const resp = await fetch(closePaneEndpoint(windowId, paneId), { method: 'DELETE' });
    if (!resp.ok) {
      console.warn('close pane failed', { windowId, paneId, status: resp.status });
    }
  } catch (e) {
    console.warn('close pane request errored', { windowId, paneId, error: e });
  }
}
