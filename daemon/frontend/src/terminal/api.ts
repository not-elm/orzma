export const SESSIONS_ENDPOINT = '/sessions';
export const sessionEndpoint = (sessionId: string) => `/sessions/${sessionId}`;
export const sessionWindowsEndpoint = (sessionId: string) => `/sessions/${sessionId}/windows`;
export const windowEndpoint = (sessionId: string, windowId: string) =>
  `/sessions/${sessionId}/windows/${windowId}`;
export const windowSelectEndpoint = (sessionId: string, windowId: string) =>
  `/sessions/${sessionId}/windows/${windowId}/select`;
export const splitPaneEndpoint = (sessionId: string, windowId: string, paneId: string) =>
  `/sessions/${sessionId}/windows/${windowId}/panes/${paneId}/split`;
export const closePaneEndpoint = (sessionId: string, windowId: string, paneId: string) =>
  `/sessions/${sessionId}/windows/${windowId}/panes/${paneId}`;
export const terminalWsUrl = (activityId: string) => {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}/activities/${activityId}/terminal/ws`;
};
