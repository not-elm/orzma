export const SESSIONS_ENDPOINT = '/sessions';
export const sessionEndpoint = (sid: string) => `/sessions/${sid}`;

export const WINDOWS_ENDPOINT = '/windows';
export const windowEndpoint = (wid: string) => `/windows/${wid}`;
export const windowSelectEndpoint = (wid: string) => `/windows/${wid}/select`;

export const splitPaneEndpoint = (wid: string, pid: string) => `/windows/${wid}/panes/${pid}/split`;
export const closePaneEndpoint = (wid: string, pid: string) => `/windows/${wid}/panes/${pid}`;
export const addActivityEndpoint = (wid: string, pid: string) =>
  `/windows/${wid}/panes/${pid}/activities`;
export const activateActivityEndpoint = (wid: string, pid: string, aid: string) =>
  `/windows/${wid}/panes/${pid}/activities/${aid}/activate`;
export const closeActivityEndpoint = (wid: string, pid: string, aid: string) =>
  `/windows/${wid}/panes/${pid}/activities/${aid}`;
export const breakActivityToPaneEndpoint = (wid: string, pid: string, aid: string) =>
  `/windows/${wid}/panes/${pid}/activities/${aid}/break-to-pane`;
export const cycleActivityEndpoint = (wid: string, pid: string) =>
  `/windows/${wid}/panes/${pid}/cycle-activity`;
export const focusPaneEndpoint = (wid: string) => `/windows/${wid}/focus-pane`;
export const resizePaneEndpoint = (wid: string, pid: string) =>
  `/windows/${wid}/panes/${pid}/resize`;
export const swapPaneEndpoint = (wid: string, pid: string) => `/windows/${wid}/panes/${pid}/swap`;
export const windowDimensionsEndpoint = (wid: string) => `/windows/${wid}/dimensions`;

/** Options for {@link vtTerminalWsUrl}. */
export interface VtWsUrlOpts {
  lastSeq?: number;
  replay?: string;
  recordPerf?: boolean;
}

/** Builds the VT terminal WebSocket URL, encoding optional replay/perf flags as query params. */
export const vtTerminalWsUrl = (
  windowId: string,
  paneId: string,
  activityId: string,
  opts: VtWsUrlOpts = {},
) => {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const base = `${proto}//${location.host}/windows/${windowId}/panes/${paneId}/activities/${activityId}/terminal/ws`;
  const params = new URLSearchParams();
  if (typeof opts.lastSeq === 'number') params.set('last_seq', String(opts.lastSeq));
  if (opts.replay) params.set('replay', opts.replay);
  if (opts.recordPerf) params.set('record-perf', '1');
  const q = params.toString();
  return q ? `${base}?${q}` : base;
};
