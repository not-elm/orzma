export const WINDOWS_ENDPOINT = '/windows';
export const windowEndpoint = (wid: string) => `/windows/${wid}`;

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

export const vtTerminalWsUrl = (
  windowId: string,
  paneId: string,
  activityId: string,
  lastSeq?: number,
) => {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const base = `${proto}//${location.host}/windows/${windowId}/panes/${paneId}/activities/${activityId}/terminal/ws`;
  if (typeof lastSeq !== 'number') return base;
  const params = new URLSearchParams({ last_seq: String(lastSeq) });
  return `${base}?${params.toString()}`;
};
