import { useEffect, useState } from 'react';
import { fetchJson } from '../fetchJson';

export type DefaultWindowState =
  | { status: 'loading' }
  | { status: 'ready'; windowId: string }
  | { status: 'error'; message: string };

export function useDefaultWindow(): DefaultWindowState {
  const [state, setState] = useState<DefaultWindowState>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = (await fetchJson('/sessions')) as { sessions: Array<{ id: string }> };
        const sessionId = list.sessions[0]?.id;
        if (!sessionId) throw new Error('no default session');
        const session = (await fetchJson(`/sessions/${sessionId}`)) as {
          linkedWindows: string[];
          active_window: string | null;
        };
        const windowId = session.active_window ?? session.linkedWindows[0];
        if (!windowId) throw new Error('no default window');
        if (!cancelled) setState({ status: 'ready', windowId });
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
