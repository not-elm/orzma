export const SESSIONS_ENDPOINT = '/sessions';
export const sessionEndpoint = (sid: string) => `/sessions/${sid}`;

export const WINDOWS_ENDPOINT = '/windows';
export const windowEndpoint = (wid: string) => `/windows/${wid}`;
export const windowSelectEndpoint = (wid: string) => `/windows/${wid}/select`;

export const splitPaneEndpoint = (pid: string) => `/panes/${pid}/split`;
export const closePaneEndpoint = (pid: string) => `/panes/${pid}`;

export const terminalWsUrl = (activityId: string) => {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}/activities/${activityId}/terminal/ws`;
};
