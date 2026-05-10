import { useEffect, useRef, useState } from 'react';
import { windowEventsWsUrl } from './api';
import type { WindowView } from './types';

export type LayoutState =
  | { status: 'connecting'; view: WindowView | null }
  | { status: 'live'; view: WindowView }
  | { status: 'reconnecting'; view: WindowView | null; retryInSec: number }
  | { status: 'gone'; reason: 'window_not_found' | 'window_closed' };

export function useWindowLayout(wid: string | null): LayoutState {
  const [state, setState] = useState<LayoutState>({ status: 'connecting', view: null });
  const generationRef = useRef(0);

  useEffect(() => {
    if (wid === null) {
      setState({ status: 'connecting', view: null });
      return;
    }
    const myGen = ++generationRef.current;
    const ws = new WebSocket(windowEventsWsUrl(wid));

    ws.onmessage = (ev) => {
      if (generationRef.current !== myGen) return;
      try {
        const view = JSON.parse(typeof ev.data === 'string' ? ev.data : '') as WindowView;
        setState({ status: 'live', view });
      } catch {
        // malformed JSON — ignore
      }
    };

    return () => {
      generationRef.current++;
      ws.close();
    };
  }, [wid]);

  return state;
}
