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
    setState({ status: 'connecting', view: null });
    lastViewRef.current = null;
    const myGen = ++generationRef.current;
    attemptRef.current = 0;

    let activeWs: WebSocket | null = null;
    let pendingTimer: ReturnType<typeof setTimeout> | null = null;
    let resumeListener: (() => void) | null = null;

    const scheduleReconnect = (delay: number) => {
      if (document.hidden) {
        resumeListener = () => {
          if (resumeListener) {
            document.removeEventListener('visibilitychange', resumeListener);
            resumeListener = null;
          }
          if (generationRef.current !== myGen) return;
          if (document.hidden) return;
          connect();
        };
        document.addEventListener('visibilitychange', resumeListener);
        return;
      }
      if (delay === 0) {
        connect();
        return;
      }
      pendingTimer = setTimeout(() => {
        pendingTimer = null;
        if (generationRef.current !== myGen) return;
        connect();
      }, delay);
    };

    const connect = () => {
      const ws = new WebSocket(windowEventsWsUrl(wid));
      activeWs = ws;
      ws.onmessage = (ev) => {
        if (generationRef.current !== myGen) return;
        try {
          const view = JSON.parse(typeof ev.data === 'string' ? ev.data : '') as WindowView;
          attemptRef.current = 0;
          lastViewRef.current = view;
          setState({ status: 'live', view });
        } catch {
          /* ignore */
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
          RECONNECT_RECOVERABLE_CODES.has(ev.code) || RECONNECT_RECOVERABLE_REASONS.has(ev.reason);
        if (!recoverable) return;
        attemptRef.current++;
        const logFn: (...args: unknown[]) => void =
          attemptRef.current <= 5 ? console.warn : console.debug;
        logFn('[useWindowLayout] reconnect', {
          attempt: attemptRef.current,
          prevCloseCode: ev.code,
          prevReason: ev.reason,
        });
        if (attemptRef.current === 1) {
          setState({ status: 'reconnecting', view: lastViewRef.current, retryInSec: 0 });
          scheduleReconnect(0);
        } else {
          const baseDelay = Math.min(30_000, 500 * 2 ** (attemptRef.current - 2));
          const jitter = Math.random() * 500;
          const delay = baseDelay + jitter;
          setState({
            status: 'reconnecting',
            view: lastViewRef.current,
            retryInSec: delay / 1000,
          });
          scheduleReconnect(delay);
        }
      };
    };

    connect();

    return () => {
      generationRef.current++;
      if (pendingTimer !== null) {
        clearTimeout(pendingTimer);
        pendingTimer = null;
      }
      if (resumeListener) {
        document.removeEventListener('visibilitychange', resumeListener);
        resumeListener = null;
      }
      if (activeWs && activeWs.readyState !== WebSocket.CLOSED) {
        activeWs.close();
      }
      activeWs = null;
    };
  }, [wid]);

  return state;
}
