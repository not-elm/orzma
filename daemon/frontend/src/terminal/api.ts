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

export const terminalWsUrl = (windowId: string, paneId: string, activityId: string) => {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}/windows/${windowId}/panes/${paneId}/activities/${activityId}/terminal/ws`;
};
