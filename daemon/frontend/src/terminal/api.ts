export const SESSIONS_ENDPOINT = "/sessions";
export const sessionEndpoint = (sessionId: string) => `/sessions/${sessionId}`;
export const terminalWsUrl = (activityId: string) => {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${location.host}/activities/${activityId}/terminal/ws`;
};
