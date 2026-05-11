import type { PaneId } from './types';

export function useClosePane(): (paneId: PaneId) => Promise<void> {
  return async (paneId: PaneId) => {
    try {
      const resp = await fetch(`/panes/${paneId}`, { method: 'DELETE' });
      if (!resp.ok) {
        console.warn('close pane failed', { paneId, status: resp.status });
      }
    } catch (e) {
      console.warn('close pane request errored', { paneId, error: e });
    }
  };
}
