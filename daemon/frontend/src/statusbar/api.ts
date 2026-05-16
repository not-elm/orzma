import type { SessionId } from '../layout/types';

/**
 * URL of the per-session events WebSocket on the daemon's local origin.
 */
export const sessionEventsWsUrl = (sid: SessionId): string => {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${proto}//${location.host}/sessions/${sid}/events`;
};
