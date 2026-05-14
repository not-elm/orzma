import { act, renderHook, waitFor } from '@testing-library/react';
import { Server } from 'mock-socket';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { useTerminalSocket } from './useTerminalSocket';

const ENDPOINT = `ws://${location.host}/windows/w/panes/p/activities/a/terminal/ws?mode=vt&vt_version=vt-1`;

let server: Server | null = null;

beforeEach(() => {
  server = new Server(ENDPOINT);
});

afterEach(() => {
  server?.close();
  server = null;
});

describe('useTerminalSocket', () => {
  it('returns ref-stable identity across renders', () => {
    const { result, rerender } = renderHook(() => useTerminalSocket('w', 'p', 'a'));
    const first = result.current;
    rerender();
    expect(result.current).toBe(first);
  });

  it('routes Binary frames to setFrameHandler', async () => {
    const { result } = renderHook(() => useTerminalSocket('w', 'p', 'a'));
    const received: Uint8Array[] = [];
    await waitFor(() => expect(server?.clients().length).toBe(1));

    act(() => {
      result.current.setFrameHandler((bytes) => {
        received.push(bytes);
      });
    });

    const payload = new Uint8Array([1, 2, 3]);
    act(() => {
      server?.clients()[0].send(payload.buffer);
    });

    await waitFor(() => expect(received.length).toBe(1));
    expect(Array.from(received[0])).toEqual([1, 2, 3]);
  });

  it('routes Text frames to setControlHandler', async () => {
    const { result } = renderHook(() => useTerminalSocket('w', 'p', 'a'));
    const received: string[] = [];
    await waitFor(() => expect(server?.clients().length).toBe(1));

    act(() => {
      result.current.setControlHandler((text) => {
        received.push(text);
      });
    });

    act(() => {
      server?.clients()[0].send('{"kind":"mode","seq":1,"added":["alt-screen"],"removed":[]}');
    });

    await waitFor(() => expect(received.length).toBe(1));
    expect(received[0]).toContain('"kind":"mode"');
  });

  it('buffers binary frames received before setFrameHandler is registered', async () => {
    const { result } = renderHook(() => useTerminalSocket('w', 'p', 'a'));
    await waitFor(() => expect(server?.clients().length).toBe(1));

    act(() => {
      server?.clients()[0].send(new Uint8Array([9]).buffer);
    });

    const received: Uint8Array[] = [];
    act(() => {
      result.current.setFrameHandler((b) => received.push(b));
    });

    await waitFor(() => expect(received.length).toBe(1));
    expect(Array.from(received[0])).toEqual([9]);
  });
});
