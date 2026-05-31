import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as channelsServer from './channels-server.ts';
import { __resetActivityChannelsForTests } from './channels-server.ts';
import * as daemonClient from './daemon-client.ts';
import * as handlersServer from './handlers-server.ts';
import { __resetActivityHandlersForTests } from './handlers-server.ts';
import { __resetExtensionNameCacheForTests, Pane } from './pane.ts';

function tmpSock(): string {
  return path.join(
    os.tmpdir(),
    `ozmux-pane-test-${process.pid}-${Math.random().toString(36).slice(2)}.sock`,
  );
}

let postJsonSpy: ReturnType<typeof vi.spyOn>;
let savedExtensionName: string | undefined;
let savedControlSock: string | undefined;

beforeEach(() => {
  __resetActivityHandlersForTests();
  __resetActivityChannelsForTests();
  postJsonSpy = vi.spyOn(daemonClient, 'postJson').mockResolvedValue({});
  savedExtensionName = process.env.EXTENSION_NAME;
  process.env.EXTENSION_NAME = 'memo';
  __resetExtensionNameCacheForTests();
  savedControlSock = process.env.OZMUX_CONTROL_SOCK_PATH;
});

afterEach(() => {
  postJsonSpy.mockRestore();
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

/** Starts a one-shot UDS server that replies with the given new_pane_id/new_activity_id. */
async function startFakeSplitServer(
  sock: string,
  reply: { new_pane_id: string; new_activity_id: string } | 'error',
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
    const { server, frames } = await startFakeSplitServer(sock, {
      new_pane_id: 'p99',
      new_activity_id: 'a99',
    });
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const pane = new Pane({ id: 'p1', windowId: 'w1', sessionId: 's1' });
    const next = await pane.split({
      side: 'after',
      orientation: 'horizontal',
      activity: { kind: 'terminal' },
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
    expect(postJsonSpy).not.toHaveBeenCalled();
    server.close();
  });

  it('registers handlers and channels BEFORE the control call resolves (race-free)', async () => {
    const sock = tmpSock();
    const { server } = await startFakeSplitServer(sock, {
      new_pane_id: 'p2',
      new_activity_id: 'a2',
    });
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const registerHandlersSpy = vi.spyOn(handlersServer, 'registerActivityHandlers');
    const registerChannelsSpy = vi.spyOn(channelsServer, 'registerActivityChannels');
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
      activity: {
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

    // registries must be primed before the async control call completes
    expect(callOrder[0]).toBe('register-handlers');
    expect(callOrder[1]).toBe('register-channels');
    registerHandlersSpy.mockRestore();
    registerChannelsSpy.mockRestore();
    server.close();
  });

  it('rolls registries back when the control call fails', async () => {
    const sock = tmpSock();
    const { server } = await startFakeSplitServer(sock, 'error');
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const unregisterHandlersSpy = vi.spyOn(handlersServer, 'unregisterActivityHandlers');
    const unregisterChannelsSpy = vi.spyOn(channelsServer, 'unregisterActivityChannels');

    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await expect(
      pane.split({
        side: 'after',
        orientation: 'horizontal',
        activity: {
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
    unregisterHandlersSpy.mockRestore();
    unregisterChannelsSpy.mockRestore();
    server.close();
  });

  it('encodes extension html_root as the parent dir of `html` in the control frame', async () => {
    const sock = tmpSock();
    const { server, frames } = await startFakeSplitServer(sock, {
      new_pane_id: 'p3',
      new_activity_id: 'a3',
    });
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await pane.split({
      side: 'before',
      orientation: 'vertical',
      activity: {
        kind: 'extension',
        html: '/opt/memo/index.html',
      },
    });

    const params = frames[0].params as { activity: { kind: string; html_root: string } };
    expect(params.activity.kind).toBe('extension');
    expect(params.activity.html_root).toBe('/opt/memo');
    server.close();
  });

  it('split sends the client activity_id in the control frame', async () => {
    const sock = tmpSock();
    let seen: any;
    const server = net.createServer((conn) => {
      conn.on('data', (chunk) => {
        seen = JSON.parse(chunk.toString('utf8').trim());
        conn.write(
          `${JSON.stringify({
            kind: 'result',
            id: seen.id,
            payload: { new_pane_id: 'p1', new_activity_id: 'a1' },
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
      activity: { kind: 'extension', html: '/x/memo/index.html' },
    });
    expect(typeof seen.params.activity.activity_id).toBe('string');
    expect(seen.params.activity.activity_id.length).toBeGreaterThan(0);
    server.close();
  });

  it('does not require EXTENSION_NAME for terminal-kind activities (no-op fallback)', async () => {
    delete process.env.EXTENSION_NAME;
    delete process.env.OZMUX_CONTROL_SOCK_PATH;
    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await expect(
      pane.split({
        side: 'after',
        orientation: 'horizontal',
        activity: { kind: 'terminal' },
      }),
    ).resolves.toBeDefined();
  });
});

describe('Pane.addActivity', () => {
  it('POSTs to /panes/:pid/activities and returns an Activity handle', async () => {
    const pane = new Pane({ id: 'p1', windowId: 'w1', sessionId: 's1' });
    const activity = await pane.addActivity({ kind: 'terminal' });
    expect(postJsonSpy).toHaveBeenCalledTimes(1);
    const [url] = postJsonSpy.mock.calls[0] as [string, unknown];
    expect(url).toBe('/windows/w1/panes/p1/activities');
    expect(activity.paneId).toBe('p1');
    expect(activity.windowId).toBe('w1');
    expect(activity.sessionId).toBe('s1');
    expect(activity.kind).toEqual({ type: 'terminal' });
  });

  it('forwards EXTENSION_NAME on extension-kind activities', async () => {
    const pane = new Pane({ id: 'p1', windowId: 'w1' });
    await pane.addActivity({
      kind: 'extension',
      html: '/opt/memo/index.html',
    });
    const [, body] = postJsonSpy.mock.calls[0] as [string, Record<string, unknown>];
    const activity = body.activity as {
      kind: { type: string; html_root: string; extension_name: string };
    };
    expect(activity.kind).toEqual({
      type: 'extension',
      html_root: '/opt/memo',
      extension_name: 'memo',
    });
  });
});
