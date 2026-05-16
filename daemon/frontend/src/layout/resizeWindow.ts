import { windowDimensionsEndpoint } from '../terminal/api';
import type { WindowId } from './types';

/** PATCHes the daemon with new cell dimensions for the window. Failures are
 *  logged via `console.warn` and swallowed; callers do not await success. */
export async function resizeWindow(windowId: WindowId, cols: number, rows: number): Promise<void> {
  try {
    const resp = await fetch(windowDimensionsEndpoint(windowId), {
      method: 'PATCH',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ cols, rows }),
    });
    if (!resp.ok) {
      console.warn('resizeWindow failed', { windowId, cols, rows, status: resp.status });
    }
  } catch (e) {
    console.warn('resizeWindow request errored', { windowId, cols, rows, error: e });
  }
}
