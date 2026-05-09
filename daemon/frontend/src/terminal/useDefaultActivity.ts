import { useEffect, useState } from 'react';
import { SESSIONS_ENDPOINT, sessionEndpoint, windowEndpoint } from './api';

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
          sessions: Array<{ id: string }>;
        };
        const sessionId = list.sessions[0]?.id;
        if (!sessionId) throw new Error('no default session');

        const session = (await fetchJson(sessionEndpoint(sessionId))) as {
          windows: string[];
          active_window: string | null;
        };
        const windowId = session.active_window ?? session.windows[0];
        if (!windowId) throw new Error('no default window');

        const win = (await fetchJson(windowEndpoint(windowId))) as {
          active_pane: string;
          panes: Array<{ id: string; active_activity: string }>;
        };
        const activePane = win.panes.find((p) => p.id === win.active_pane) ?? win.panes[0];
        const activityId = activePane?.active_activity;
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
