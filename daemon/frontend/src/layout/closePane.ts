import { closePaneEndpoint } from '../terminal/api';
import type { PaneId } from './types';

export async function closePane(paneId: PaneId): Promise<void> {
  try {
    const resp = await fetch(closePaneEndpoint(paneId), { method: 'DELETE' });
    if (!resp.ok) {
      console.warn('close pane failed', { paneId, status: resp.status });
    }
  } catch (e) {
    console.warn('close pane request errored', { paneId, error: e });
  }
}
