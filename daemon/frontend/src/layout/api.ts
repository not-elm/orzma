export const windowEventsWsUrl = (windowId: string): string => {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}/windows/${windowId}/events`;
};
