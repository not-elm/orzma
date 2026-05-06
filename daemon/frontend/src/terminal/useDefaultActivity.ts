import { useEffect, useState } from 'react';
import { SESSIONS_ENDPOINT, sessionEndpoint } from './api';

export type DefaultActivityState =
  | { status: 'loading' }
  | { status: 'ready'; activityId: string }
  | { status: 'error'; message: string };

async function fetchJson(url: string): Promise<unknown> {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: ${r.status} ${r.statusText}`);
  return r.json();
}

export function useDefaultActivity(): DefaultActivityState {
  const [state, setState] = useState<DefaultActivityState>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = (await fetchJson(SESSIONS_ENDPOINT)) as {
          sessions?: Array<{ id?: string }>;
        };
        const sessionId = list.sessions?.[0]?.id;
        if (!sessionId) throw new Error('no default session');
        const session = (await fetchJson(sessionEndpoint(sessionId))) as {
          windows?: Array<{
            panes?: Array<{ activities?: Array<{ id?: string }> }>;
          }>;
        };
        const activityId = session.windows?.[0]?.panes?.[0]?.activities?.[0]?.id;
        if (!activityId) throw new Error('no default activity');
        if (!cancelled) setState({ status: 'ready', activityId });
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
