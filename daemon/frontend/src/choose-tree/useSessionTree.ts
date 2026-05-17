import { useEffect, useState } from 'react';
import { fetchJson } from '../fetchJson';
import type { SessionTreeNode, SessionTreeState } from './types';

/**
 * Fetches `GET /sessions/tree` once whenever `active` toggles from
 * false to true (i.e. each time the picker opens). The hook does not
 * subscribe to live updates — the spec ships without them.
 */
export function useSessionTree(active: boolean): SessionTreeState {
  const [state, setState] = useState<SessionTreeState>({ status: 'loading' });

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    setState({ status: 'loading' });
    (async () => {
      try {
        const raw = (await fetchJson('/sessions/tree')) as { sessions: SessionTreeNode[] };
        if (!cancelled) setState({ status: 'ready', tree: raw.sessions });
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
  }, [active]);

  return state;
}
