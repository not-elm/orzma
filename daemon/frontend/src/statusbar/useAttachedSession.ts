import { useEffect, useState } from 'react';
import { fetchJson } from '../fetchJson';
import type { SessionId } from '../layout/types';

/** Public state of `useAttachedSession`. */
export type AttachedSessionState =
  | { status: 'loading' }
  | { status: 'ready'; sessionId: SessionId; setSession: (sid: SessionId) => void }
  | { status: 'error'; message: string };

/**
 * Resolves the initial attached session by reading the first entry from
 * `GET /sessions`, then exposes `setSession` so the choose-tree picker
 * can switch to a different session at runtime. Downstream hooks
 * (`useSessionView`, `useWindowLayout`) re-subscribe automatically when
 * the returned `sessionId` changes.
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
        const list = (await fetchJson('/sessions')) as {
          sessions: Array<{ id: SessionId }>;
        };
        const initial = list.sessions[0]?.id;
        if (!initial) throw new Error('no default session');
        if (!cancelled) {
          setSessionId(initial);
          setBootState({ status: 'ready' });
        }
      } catch (e) {
        if (!cancelled) {
          const message = e instanceof Error ? e.message : String(e);
          setBootState({ status: 'error', message });
        }
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
