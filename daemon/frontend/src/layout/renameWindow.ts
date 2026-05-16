import { windowEndpoint } from '../terminal/api';
import type { WindowId } from './types';

/**
 * Renames `wid` to `name` via `PATCH /windows/{wid}`. Best-effort —
 * non-OK responses and thrown errors are warned to the console and
 * swallowed (the next session-view broadcast is the source of truth).
 * The caller passes a trimmed, non-empty name.
 */
export async function renameWindow(wid: WindowId, name: string): Promise<void> {
  try {
    const resp = await fetch(windowEndpoint(wid), {
      method: 'PATCH',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name }),
    });
    if (!resp.ok) {
      console.warn('window rename failed', { wid, status: resp.status });
    }
  } catch (e) {
    console.warn('window rename request errored', { wid, error: e });
  }
}
