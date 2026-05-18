import { useEffect, useState } from 'react';
import { fetchJson } from '../fetchJson';
import type { SessionId } from '../layout/types';

const DEEP_LINK_RETRY_INTERVAL_MS = 200;
const DEEP_LINK_RETRY_ATTEMPTS = 3;

/** Public state of `useAttachedSession`. */
export type AttachedSessionState =
  | { status: 'loading' }
  | { status: 'ready'; sessionId: SessionId; setSession: (sid: SessionId) => void }
  | { status: 'error'; message: string };

interface SessionsListResponse {
  sessions: Array<{ id: SessionId }>;
}

/**
 * Resolves the initial attached session. Reads `?session=<id>` from the
 * current URL when present and retries `GET /sessions` briefly so a
 * just-created session has time to propagate to the broadcast. Falls
 * back to the first session in the list when the requested id never
 * appears.
 */
export function useAttachedSession(): AttachedSessionState {
  const [bootState, setBootState] = useState<
    { status: 'loading' } | { status: 'error'; message: string } | { status: 'ready' }
  >({ status: 'loading' });
  const [sessionId, setSessionId] = useState<SessionId | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const requested = readRequestedSessionId();
        const resolved = await resolveSessionId(requested);
        if (cancelled) return;
        setSessionId(resolved);
        setBootState({ status: 'ready' });
      } catch (e) {
        if (cancelled) return;
        const message = e instanceof Error ? e.message : String(e);
        setBootState({ status: 'error', message });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  if (bootState.status === 'error') return { status: 'error', message: bootState.message };
  if (bootState.status === 'loading' || sessionId === null) return { status: 'loading' };
  return { status: 'ready', sessionId, setSession: setSessionId };
}

function readRequestedSessionId(): SessionId | null {
  try {
    return (new URLSearchParams(window.location.search).get('session') as SessionId) ?? null;
  } catch {
    return null;
  }
}

async function resolveSessionId(requested: SessionId | null): Promise<SessionId> {
  for (let attempt = 0; attempt <= DEEP_LINK_RETRY_ATTEMPTS; attempt++) {
    const list = (await fetchJson('/sessions')) as SessionsListResponse;
    if (requested) {
      const hit = list.sessions.find((s) => s.id === requested);
      if (hit) return hit.id;
      if (attempt < DEEP_LINK_RETRY_ATTEMPTS) {
        await sleep(DEEP_LINK_RETRY_INTERVAL_MS);
        continue;
      }
      console.warn(`useAttachedSession: session '${requested}' not found; falling back to first`);
    }
    const initial = list.sessions[0]?.id;
    if (!initial) throw new Error('no default session');
    return initial;
  }
  throw new Error('no default session');
}

function sleep(ms: number): Promise<void> {
  return new Promise((res) => setTimeout(res, ms));
}
