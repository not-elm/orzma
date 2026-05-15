/** Builds the WS URL for an activity's browser stream. */
export const browserWsUrl = (windowId: string, paneId: string, activityId: string): string => {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}/windows/${windowId}/panes/${paneId}/activities/${activityId}/browser/ws`;
};
