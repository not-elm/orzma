import type { SessionId, WindowId } from '../layout/types';

/**
 * One entry in `SessionView.windows`. Mirrors the backend
 * `SessionWindowEntry` in `daemon/http_server/src/session_view.rs`.
 */
export interface SessionWindowEntry {
  id: WindowId;
  name: string;
  index: number;
}

/**
 * Snapshot of one session and the windows it owns. Mirrors backend
 * `SessionView`.
 */
export interface SessionView {
  id: SessionId;
  name: string;
  active_window: WindowId | null;
  windows: SessionWindowEntry[];
}
