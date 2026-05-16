import type { WindowId } from '../layout/types';

/**
 * Promote `wid` to active window of its parent session. Best-effort —
 * non-OK responses and thrown errors are warned to the console and
 * swallowed (the next session view update is the source of truth).
 */
export async function windowSelect(wid: WindowId): Promise<void> {
  try {
    const resp = await fetch(`/windows/${wid}/select`, { method: 'POST' });
    if (!resp.ok) {
      console.warn('window select failed', { wid, status: resp.status });
    }
  } catch (e) {
    console.warn('window select request errored', { wid, error: e });
  }
}
