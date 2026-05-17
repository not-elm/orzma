import { WINDOWS_ENDPOINT } from '../terminal/api';
import type { SessionId } from './types';

/**
 * Creates a new window attached to `sid` via `POST /windows`. Best-effort —
 * non-OK responses and thrown errors are warned to the console and
 * swallowed (the next session-view broadcast is the source of truth).
 */
export async function createWindow(sid: SessionId): Promise<void> {
  try {
    const resp = await fetch(WINDOWS_ENDPOINT, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ session_id: sid }),
    });
    if (!resp.ok) {
      console.warn('window create failed', { sid, status: resp.status });
    }
  } catch (e) {
    console.warn('window create request errored', { sid, error: e });
  }
}
