import { swapPaneEndpoint } from '../terminal/api';
import type { PaneId, WindowId } from './types';

/** POSTs a swap request; the layout update arrives over the layout broadcast WS. */
export async function swapPane(
  windowId: WindowId,
  paneId: PaneId,
  offset: 'prev' | 'next',
): Promise<void> {
  try {
    const resp = await fetch(swapPaneEndpoint(windowId, paneId), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ offset }),
    });
    if (!resp.ok) {
      console.warn('swap pane failed', { windowId, paneId, offset, status: resp.status });
    }
  } catch (e) {
    console.warn('swap pane request errored', { windowId, paneId, offset, error: e });
  }
}
