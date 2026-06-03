import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as channelsServer from './channels-server.ts';
import { __resetSurfaceChannelsForTests } from './channels-server.ts';
import * as controlClient from './control-client.ts';
import * as handlersServer from './handlers-server.ts';
import { __resetSurfaceHandlersForTests } from './handlers-server.ts';
import {
  __resetExtensionNameCacheForTests,
  __test_controlSurface as controlSurface,
  Pane,
} from './pane.ts';
import { Surface } from './surface.ts';

function tmpSock(): string {
  return path.join(
    os.tmpdir(),
    `ozmux-pane-test-${process.pid}-${Math.random().toString(36).slice(2)}.sock`,
  );
}

let savedExtensionName: string | undefined;
let savedControlSock: string | undefined;

beforeEach(() => {
  __resetSurfaceHandlersForTests();
  __resetSurfaceChannelsForTests();
  savedExtensionName = process.env.EXTENSION_NAME;
  process.env.EXTENSION_NAME = 'memo';
  __resetExtensionNameCacheForTests();
  savedControlSock = process.env.OZMUX_CONTROL_SOCK_PATH;
});

afterEach(() => {
  vi.restoreAllMocks();
  if (savedExtensionName === undefined) {
    delete process.env.EXTENSION_NAME;
  } else {
    process.env.EXTENSION_NAME = savedExtensionName;
  }
  __resetExtensionNameCacheForTests();
  if (savedControlSock === undefined) {
    delete process.env.OZMUX_CONTROL_SOCK_PATH;
  } else {
    process.env.OZMUX_CONTROL_SOCK_PATH = savedControlSock;
  }
});

/** Starts a one-shot UDS server that replies with the given payload. */
async function startFakeControlServer(
  sock: string,
  reply: Record<string, unknown> | 'error',
): Promise<{ server: net.Server; frames: Array<Record<string, unknown>> }> {
  const frames: Array<Record<string, unknown>> = [];
  const server = net.createServer((conn) => {
    conn.on('data', (chunk) => {
      const frame = JSON.parse(chunk.toString('utf8').trim()) as Record<string, unknown>;
      frames.push(frame);
      if (reply === 'error') {
        conn.write(
          `${JSON.stringify({ kind: 'error', id: frame.id, code: 'boom', message: 'boom' })}\n`,
        );
      } else {
        conn.write(`${JSON.stringify({ kind: 'result', id: frame.id, payload: reply })}\n`);
      }
    });
  });
  await new Promise<void>((r) => server.listen(sock, r));
  return { server, frames };
}

describe('Pane.split', () => {
  it('sends a split call over the control socket and returns the host-authoritative pane id', async () => {
    const sock = tmpSock();
    const { server, frames } = await startFakeControlServer(sock, {
      new_pane_id: 'p99',
      new_surface_id: 'a99',
    });
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const pane = new Pane({ id: 'p1', windowId: 'w1', sessionId: 's1' });
    const next = await pane.split({
      side: 'after',
      orientation: 'horizontal',
      surface: { kind: 'terminal' },
    });

    expect(frames).toHaveLength(1);
    expect(frames[0].kind).toBe('call');
    expect(frames[0].op).toBe('split');
    expect(frames[0].pane).toBe('p1');
    expect((frames[0].params as Record<string, unknown>).side).toBe('after');
    expect((frames[0].params as Record<string, unknown>).orientation).toBe('horizontal');
    expect(next.id).toBe('p99');
    expect(next.windowId).toBe('w1');
    expect(next.sessionId).toBe('s1');
    server.close();
  });

  it('registers handlers and channels BEFORE the control call resolves (race-free)', async () => {
    const sock = tmpSock();
    const { server } = await startFakeControlServer(sock, {
      new_pane_id: 'p2',
      new_surface_id: 'a2',
    });
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const registerHandlersSpy = vi.spyOn(handlersServer, 'registerSurfaceHandlers');
    const registerChannelsSpy = vi.spyOn(channelsServer, 'registerSurfaceChannels');
    const callOrder: string[] = [];
    registerHandlersSpy.mockImplementation(() => {
      callOrder.push('register-handlers');
    });
    registerChannelsSpy.mockImplementation(() => {
      callOrder.push('register-channels');
    });

    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await pane.split({
      side: 'after',
      orientation: 'horizontal',
      surface: {
        kind: 'extension',
        html: '/tmp/index.html',
        handlers: { greet: async () => ({}) },
        channels: {
          tick: async function* () {
            yield 1;
          },
        },
      },
    });

    expect(callOrder[0]).toBe('register-handlers');
    expect(callOrder[1]).toBe('register-channels');
    server.close();
  });

  it('rolls registries back when the control call fails', async () => {
    const sock = tmpSock();
    const { server } = await startFakeControlServer(sock, 'error');
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const unregisterHandlersSpy = vi.spyOn(handlersServer, 'unregisterSurfaceHandlers');
    const unregisterChannelsSpy = vi.spyOn(channelsServer, 'unregisterSurfaceChannels');

    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await expect(
      pane.split({
        side: 'after',
        orientation: 'horizontal',
        surface: {
          kind: 'extension',
          html: '/tmp/index.html',
          handlers: { greet: async () => ({}) },
          channels: {
            tick: async function* () {
              yield 1;
            },
          },
        },
      }),
    ).rejects.toThrow(/boom/);

    expect(unregisterHandlersSpy).toHaveBeenCalledTimes(1);
    expect(unregisterChannelsSpy).toHaveBeenCalledTimes(1);
    expect(unregisterHandlersSpy.mock.calls[0][0]).toBe(unregisterChannelsSpy.mock.calls[0][0]);
    server.close();
  });

  it('encodes the extension entry as the html path relative to cwd in the control frame', async () => {
    const sock = tmpSock();
    const { server, frames } = await startFakeControlServer(sock, {
      new_pane_id: 'p3',
      new_surface_id: 'a3',
    });
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const html = path.join(process.cwd(), 'ui', 'app.html');
    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await pane.split({
      side: 'before',
      orientation: 'vertical',
      surface: {
        kind: 'extension',
        html,
      },
    });

    const params = frames[0].params as {
      surface: { kind: string; entry: string; extension_name: string };
    };
    expect(params.surface.kind).toBe('extension');
    expect(params.surface.entry).toBe('ui/app.html');
    expect(params.surface.extension_name).toBe('memo');
    server.close();
  });

  it('split sends the client surface_id in the control frame', async () => {
    const sock = tmpSock();
    let seen: Record<string, unknown> | undefined;
    const server = net.createServer((conn) => {
      conn.on('data', (chunk) => {
        seen = JSON.parse(chunk.toString('utf8').trim()) as Record<string, unknown>;
        conn.write(
          `${JSON.stringify({
            kind: 'result',
            id: seen.id,
            payload: { new_pane_id: 'p1', new_surface_id: 'a1' },
          })}\n`,
        );
      });
    });
    await new Promise<void>((r) => server.listen(sock, r));
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;
    process.env.EXTENSION_NAME = 'memo';
    const pane = new Pane({ id: '100', windowId: 'w', sessionId: 's' });
    await pane.split({
      side: 'after',
      orientation: 'vertical',
      surface: { kind: 'extension', html: '/x/memo/index.html' },
    });
    const params = seen?.params as { surface: { surface_id: string } } | undefined;
    expect(typeof params?.surface?.surface_id).toBe('string');
    expect((params?.surface?.surface_id ?? '').length).toBeGreaterThan(0);
    server.close();
  });

  it('does not require EXTENSION_NAME for terminal-kind surfaces (no-op fallback)', async () => {
    delete process.env.EXTENSION_NAME;
    delete process.env.OZMUX_CONTROL_SOCK_PATH;
    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await expect(
      pane.split({
        side: 'after',
        orientation: 'horizontal',
        surface: { kind: 'terminal' },
      }),
    ).resolves.toBeDefined();
  });

  it('sends a browser split carrying the raw url, no extension_name', async () => {
    const sock = tmpSock();
    const { server, frames } = await startFakeControlServer(sock, {
      new_pane_id: 'p7',
      new_surface_id: 'a7',
    });
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await pane.split({
      side: 'after',
      orientation: 'vertical',
      surface: { kind: 'browser', url: 'github.com' },
    });

    const params = frames[0].params as {
      surface: { kind: string; url: string; surface_id: string; extension_name?: string };
    };
    expect(params.surface.kind).toBe('browser');
    expect(params.surface.url).toBe('github.com');
    expect(params.surface.extension_name).toBeUndefined();
    expect(typeof params.surface.surface_id).toBe('string');
    server.close();
  });
});

describe('Pane.addSurface', () => {
  it('calls callControl(add_surface) and returns a Surface whose id equals the host reply', async () => {
    const callControlSpy = vi.spyOn(controlClient, 'callControl').mockResolvedValue({
      new_surface_id: 'host-aid-42',
    });

    const pane = new Pane({ id: 'p1', windowId: 'w1', sessionId: 's1' });
    const surface = await pane.addSurface({ kind: 'terminal' });

    expect(callControlSpy).toHaveBeenCalledTimes(1);
    const [op, paneId] = callControlSpy.mock.calls[0] as [string, string, unknown];
    expect(op).toBe('add_surface');
    expect(paneId).toBe('p1');
    expect(surface.id).toBe('host-aid-42');
    expect(surface.paneId).toBe('p1');
    expect(surface.windowId).toBe('w1');
    expect(surface.sessionId).toBe('s1');
    expect(surface.kind).toEqual({ type: 'terminal' });
  });

  it('sends extension entry/extension_name/surface_id in the control params', async () => {
    const callControlSpy = vi.spyOn(controlClient, 'callControl').mockResolvedValue({
      new_surface_id: 'host-aid-7',
    });

    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    const html = path.join(process.cwd(), 'index.html');
    await pane.addSurface({ kind: 'extension', html });

    const [, , params] = callControlSpy.mock.calls[0] as [
      string,
      string,
      { surface: { kind: string; entry: string; extension_name: string; surface_id: string } },
    ];
    expect(params.surface.kind).toBe('extension');
    expect(params.surface.entry).toBe('index.html');
    expect(params.surface.extension_name).toBe('memo');
    expect(typeof params.surface.surface_id).toBe('string');
    expect(params.surface.surface_id.length).toBeGreaterThan(0);
  });

  it('registers channels BEFORE the callControl resolves (race-free)', async () => {
    const callOrder: string[] = [];
    const registerChannelsSpy = vi.spyOn(channelsServer, 'registerSurfaceChannels');
    registerChannelsSpy.mockImplementation(() => {
      callOrder.push('register-channels');
    });
    vi.spyOn(controlClient, 'callControl').mockImplementation(async () => {
      callOrder.push('control-call');
      return { new_surface_id: 'h1' };
    });

    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await pane.addSurface({
      kind: 'extension',
      html: '/tmp/index.html',
      channels: {
        tick: async function* () {
          yield 1;
        },
      },
    });

    expect(callOrder[0]).toBe('register-channels');
    expect(callOrder[1]).toBe('control-call');
  });

  it('rolls handler + channel registries back when callControl throws', async () => {
    vi.spyOn(controlClient, 'callControl').mockRejectedValue(new Error('boom'));
    const unregisterHandlersSpy = vi.spyOn(handlersServer, 'unregisterSurfaceHandlers');
    const unregisterChannelsSpy = vi.spyOn(channelsServer, 'unregisterSurfaceChannels');

    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await expect(
      pane.addSurface({
        kind: 'extension',
        html: '/tmp/index.html',
        handlers: { greet: async () => ({}) },
        channels: {
          tick: async function* () {
            yield 1;
          },
        },
      }),
    ).rejects.toThrow(/boom/);

    expect(unregisterHandlersSpy).toHaveBeenCalledTimes(1);
    expect(unregisterChannelsSpy).toHaveBeenCalledTimes(1);
    expect(unregisterHandlersSpy.mock.calls[0][0]).toBe(unregisterChannelsSpy.mock.calls[0][0]);
  });
});

describe('Surface.activate', () => {
  it('calls callControl(activate) with the surface paneId and id', async () => {
    const callControlSpy = vi.spyOn(controlClient, 'callControl').mockResolvedValue({});

    const surface = new Surface({
      id: 'act-9',
      paneId: 'p1',
      windowId: 'w1',
      kind: { type: 'terminal' },
    });
    await surface.activate();

    expect(callControlSpy).toHaveBeenCalledTimes(1);
    const [op, paneId, params] = callControlSpy.mock.calls[0] as [
      string,
      string,
      { surface_id: string },
    ];
    expect(op).toBe('activate');
    expect(paneId).toBe('p1');
    expect(params.surface_id).toBe('act-9');
  });
});

describe('controlSurface terminal', () => {
  it('encodes a terminal surface carrying cwd (not the extension stub)', () => {
    const out = controlSurface('sid-1', { kind: 'terminal', cwd: '/work' });
    expect(out).toEqual({ kind: 'terminal', cwd: '/work', name: undefined, surface_id: 'sid-1' });
  });
});
