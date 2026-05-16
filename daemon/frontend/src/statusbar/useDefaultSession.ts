import { useEffect, useState } from 'react';
import { fetchJson } from '../fetchJson';
import type { SessionId } from '../layout/types';

/**
 * Public state of `useDefaultSession`.
 */
export type DefaultSessionState =
  | { status: 'loading' }
  | { status: 'ready'; sessionId: SessionId }
  | { status: 'error'; message: string };

/**
 * Resolves the default session id by reading the first entry from
 * `GET /sessions`. Runs once at mount; not reactive to later session
 * lifecycle changes.
 */
export function useDefaultSession(): DefaultSessionState {
  const [state, setState] = useState<DefaultSessionState>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = (await fetchJson('/sessions')) as {
          sessions: Array<{ id: SessionId }>;
        };
        const sessionId = list.sessions[0]?.id;
        if (!sessionId) throw new Error('no default session');
        if (!cancelled) setState({ status: 'ready', sessionId });
      } catch (e) {
        if (!cancelled) {
          const message = e instanceof Error ? e.message : String(e);
          setState({ status: 'error', message });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  return state;
}
