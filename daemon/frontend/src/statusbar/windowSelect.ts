import type { WindowId } from '../layout/types';

/**
 * Promote `wid` to active window of its parent session. Returns `true`
 * on a 2xx response. Non-OK responses and thrown errors are warned to
 * the console and surface as `false`; callers can keep retrying without
 * the next session view update overriding the failure indication.
 */
export async function windowSelect(wid: WindowId): Promise<boolean> {
  try {
    const resp = await fetch(`/windows/${wid}/select`, { method: 'POST' });
    if (!resp.ok) {
      console.warn('window select failed', { wid, status: resp.status });
      return false;
    }
    return true;
  } catch (e) {
    console.warn('window select request errored', { wid, error: e });
    return false;
  }
}
