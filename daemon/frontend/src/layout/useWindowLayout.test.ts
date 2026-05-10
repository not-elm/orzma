import { act, renderHook } from '@testing-library/react';
import { Server } from 'mock-socket';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { WindowView } from './types';
import { useWindowLayout } from './useWindowLayout';

const WID = 'wid-test';
const URL = `ws://${location.host}/windows/${WID}/events`;

let server: Server;

const fakeView = (overrides: Partial<WindowView> = {}): WindowView => ({
  id: WID,
  name: 'main',
  root_cell: 'cid-root',
  active_pane: 'pid-1',
  panes: [
    { id: 'pid-1', active_activity: 'aid-1', activities: [{ id: 'aid-1', kind: 'terminal' }] },
  ],
  layout_schema_version: 1,
  layout: {
    type: 'root',
    cell_id: 'cid-root',
    child: { type: 'pane', cell_id: 'cid-pane-1', pane_id: 'pid-1' },
  },
  ...overrides,
});

beforeEach(() => {
  server = new Server(URL);
});

afterEach(() => {
  server.stop();
});

describe('useWindowLayout', () => {
  it('returns connecting/null before the first frame', () => {
    const { result } = renderHook(() => useWindowLayout(WID));
    expect(result.current.status).toBe('connecting');
    expect((result.current as { view: WindowView | null }).view).toBeNull();
  });

  it('transitions to live with the snapshot view on first frame', async () => {
    server.on('connection', (sock) => {
      sock.send(JSON.stringify(fakeView()));
    });
    const { result } = renderHook(() => useWindowLayout(WID));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    expect(result.current.status).toBe('live');
    expect((result.current as { view: WindowView }).view.id).toBe(WID);
  });

  it('returns connecting/null when wid is null', () => {
    const { result } = renderHook(() => useWindowLayout(null));
    expect(result.current.status).toBe('connecting');
    expect((result.current as { view: WindowView | null }).view).toBeNull();
  });

  it('ignores stale frames after wid change', async () => {
    const WID2 = 'wid-second';
    const URL2 = `ws://${location.host}/windows/${WID2}/events`;
    const server2 = new Server(URL2);

    let firstSocket: WebSocket | null = null;
    server.on('connection', (sock) => {
      firstSocket = sock as unknown as WebSocket;
    });
    server2.on('connection', (sock) => {
      sock.send(JSON.stringify(fakeView({ id: WID2, name: 'second' })));
    });

    const { result, rerender } = renderHook(({ id }: { id: string | null }) => useWindowLayout(id), {
      initialProps: { id: WID as string | null },
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    rerender({ id: WID2 });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });
    // Now have firstSocket from the old wid send a stale frame.
    if (firstSocket) {
      (firstSocket as unknown as { send: (data: string) => void }).send(
        JSON.stringify(fakeView({ id: WID, name: 'STALE' })),
      );
    }
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20));
    });

    expect(result.current.status).toBe('live');
    expect((result.current as { view: { id: string; name: string } }).view.id).toBe(WID2);
    expect((result.current as { view: { id: string; name: string } }).view.name).toBe('second');
    server2.stop();
  });
});
