import { useEffect, useRef, useState } from 'react';
import { windowEventsWsUrl } from './api';
import type { WindowView } from './types';

export type LayoutState =
  | { status: 'connecting'; view: WindowView | null }
  | { status: 'live'; view: WindowView }
  | { status: 'reconnecting'; view: WindowView | null; retryInSec: number }
  | { status: 'gone'; reason: 'window_not_found' | 'window_closed' };

const RECONNECT_RECOVERABLE_CODES = new Set([1006]);
const RECONNECT_RECOVERABLE_REASONS = new Set(['lagged', 'internal_error']);
const TERMINAL_REASONS = new Set(['window_not_found', 'window_closed']);

export function useWindowLayout(wid: string | null): LayoutState {
  const [state, setState] = useState<LayoutState>({ status: 'connecting', view: null });
  const generationRef = useRef(0);
  const lastViewRef = useRef<WindowView | null>(null);
  const attemptRef = useRef(0);

  useEffect(() => {
    if (wid === null) {
      setState({ status: 'connecting', view: null });
      return;
    }
    const myGen = ++generationRef.current;
    attemptRef.current = 0;

    const connect = () => {
      const ws = new WebSocket(windowEventsWsUrl(wid));

      ws.onmessage = (ev) => {
        if (generationRef.current !== myGen) return;
        try {
          const view = JSON.parse(typeof ev.data === 'string' ? ev.data : '') as WindowView;
          attemptRef.current = 0;
          lastViewRef.current = view;
          setState({ status: 'live', view });
        } catch {
          // malformed JSON — ignore
        }
      };

      ws.onclose = (ev) => {
        if (generationRef.current !== myGen) return;
        if (TERMINAL_REASONS.has(ev.reason)) {
          setState({
            status: 'gone',
            reason: ev.reason as 'window_not_found' | 'window_closed',
          });
          return;
        }
        const recoverable =
          RECONNECT_RECOVERABLE_CODES.has(ev.code) ||
          RECONNECT_RECOVERABLE_REASONS.has(ev.reason);
        if (!recoverable) return;
        attemptRef.current++;
        setState({ status: 'reconnecting', view: lastViewRef.current, retryInSec: 0 });
        // 1st reconnect = immediate. Backoff added in Task 20.
        connect();
      };
    };

    connect();

    return () => {
      generationRef.current++;
    };
  }, [wid]);

  return state;
}
