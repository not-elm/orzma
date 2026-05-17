import { useEffect, useRef, useState } from 'react';
import { sessionEventsWsUrl } from './api';
import type { SessionView } from './types';

/** Public state of `useSessionView`. */
export type SessionViewState =
  | { status: 'connecting'; view: SessionView | null }
  | { status: 'live'; view: SessionView }
  | { status: 'reconnecting'; view: SessionView | null; retryInSec: number }
  | { status: 'gone'; reason: 'session_not_found' | 'session_closed' };

/** Returns the last-known view when the hook is live or reconnecting, else null. */
export function liveOrReconnectingView(s: SessionViewState): SessionView | null {
  if (s.status === 'live') return s.view;
  if (s.status === 'reconnecting') return s.view;
  return null;
}

const RECONNECT_RECOVERABLE_CODES = new Set([1006]);
const RECONNECT_RECOVERABLE_REASONS = new Set(['lagged', 'internal_error']);
const TERMINAL_REASONS = new Set(['session_not_found', 'session_closed']);

/**
 * Subscribe to `/sessions/{sid}/events` and surface the current
 * snapshot, reconnecting on transient closes. Near-line-for-line copy
 * of `useWindowLayout`; substitutions: session WS URL, session view
 * type, session terminal reasons.
 */
export function useSessionView(sid: string | null): SessionViewState {
  const [state, setState] = useState<SessionViewState>({ status: 'connecting', view: null });
  const generationRef = useRef(0);
  const lastViewRef = useRef<SessionView | null>(null);
  const attemptRef = useRef(0);

  useEffect(() => {
    if (sid === null) {
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
      const ws = new WebSocket(sessionEventsWsUrl(sid));
      activeWs = ws;
      ws.onmessage = (ev) => {
        if (generationRef.current !== myGen) return;
        try {
          const view = JSON.parse(typeof ev.data === 'string' ? ev.data : '') as SessionView;
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
            reason: ev.reason as 'session_not_found' | 'session_closed',
          });
          return;
        }
        const recoverable =
          RECONNECT_RECOVERABLE_CODES.has(ev.code) || RECONNECT_RECOVERABLE_REASONS.has(ev.reason);
        if (!recoverable) return;
        attemptRef.current++;
        const logFn: (...args: unknown[]) => void =
          attemptRef.current <= 5 ? console.warn : console.debug;
        logFn('[useSessionView] reconnect', {
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
  }, [sid]);

  return state;
}
