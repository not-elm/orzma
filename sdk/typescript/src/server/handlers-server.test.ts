import * as fs from 'node:fs/promises';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  __resetSurfaceHandlersForTests,
  bindHandlersServer,
  registerSurfaceHandlers,
} from './handlers-server.ts';

let server: net.Server | undefined;
let sockPath = '';

beforeEach(async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'ozmux-test-'));
  sockPath = path.join(dir, 'memo.handlers.sock');
  __resetSurfaceHandlersForTests();
});

afterEach(async () => {
  if (server) {
    await new Promise<void>((res) => server?.close(() => res()));
    server = undefined;
  }
});

function connect(): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const s = net.connect(sockPath);
    s.once('connect', () => resolve(s));
    s.once('error', reject);
  });
}

async function sendLine(s: net.Socket, obj: unknown) {
  s.write(`${JSON.stringify(obj)}\n`);
}

function readOneLine(s: net.Socket): Promise<string> {
  return new Promise((resolve) => {
    let buf = '';
    s.on('data', (chunk) => {
      buf += chunk.toString('utf8');
      const idx = buf.indexOf('\n');
      if (idx !== -1) resolve(buf.slice(0, idx));
    });
  });
}

describe('bindHandlersServer', () => {
  it('listens on the given path and chmods 0600', async () => {
    server = await bindHandlersServer(sockPath);
    const stat = await fs.stat(sockPath);
    expect(stat.mode & 0o777).toBe(0o600);
  });

  it('unlinks a stale socket before listening', async () => {
    await fs.writeFile(sockPath, '');
    server = await bindHandlersServer(sockPath);
    expect(server.listening).toBe(true);
  });

  it('sets maxConnections=64', async () => {
    server = await bindHandlersServer(sockPath);
    expect(server.maxConnections).toBe(64);
  });

  it('dispatches call to registered handler and returns result frame', async () => {
    server = await bindHandlersServer(sockPath);
    registerSurfaceHandlers('aid-1', {
      greet: async (req: { name: string }) => ({
        message: `Hello, ${req.name}!`,
      }),
    });
    const s = await connect();
    await sendLine(s, {
      surface_id: 'aid-1',
      frame: { kind: 'call', id: '1', name: 'greet', payload: { name: 'x' } },
    });
    const line = await readOneLine(s);
    const env = JSON.parse(line);
    expect(env.surface_id).toBe('aid-1');
    expect(env.frame).toEqual({
      kind: 'result',
      id: '1',
      payload: { message: 'Hello, x!' },
    });
    s.destroy();
  });

  it('returns UNKNOWN_HANDLER error frame for unregistered handler', async () => {
    server = await bindHandlersServer(sockPath);
    registerSurfaceHandlers('aid-1', {});
    const s = await connect();
    await sendLine(s, {
      surface_id: 'aid-1',
      frame: { kind: 'call', id: '9', name: 'ghost', payload: {} },
    });
    const line = await readOneLine(s);
    const env = JSON.parse(line);
    expect(env.frame.kind).toBe('error');
    expect(env.frame.code).toBe('UNKNOWN_HANDLER');
    expect(env.frame.id).toBe('9');
    s.destroy();
  });

  it('returns HANDLER_ERROR frame when handler throws', async () => {
    server = await bindHandlersServer(sockPath);
    registerSurfaceHandlers('aid-1', {
      boom: async () => {
        throw new Error('nope');
      },
    });
    const s = await connect();
    await sendLine(s, {
      surface_id: 'aid-1',
      frame: { kind: 'call', id: '2', name: 'boom', payload: {} },
    });
    const line = await readOneLine(s);
    const env = JSON.parse(line);
    expect(env.frame.kind).toBe('error');
    expect(env.frame.code).toBe('HANDLER_ERROR');
    expect(env.frame.message).toContain('nope');
    s.destroy();
  });

  it('handles multiple sequential calls on the same connection', async () => {
    server = await bindHandlersServer(sockPath);
    registerSurfaceHandlers('aid-1', {
      echo: async (req: { v: number }) => ({ v: req.v }),
    });
    const s = await connect();
    const lines: string[] = [];
    let buf = '';
    s.on('data', (chunk) => {
      buf += chunk.toString('utf8');
      while (true) {
        const idx = buf.indexOf('\n');
        if (idx === -1) break;
        lines.push(buf.slice(0, idx));
        buf = buf.slice(idx + 1);
      }
    });
    await sendLine(s, {
      surface_id: 'aid-1',
      frame: { kind: 'call', id: '1', name: 'echo', payload: { v: 1 } },
    });
    await sendLine(s, {
      surface_id: 'aid-1',
      frame: { kind: 'call', id: '2', name: 'echo', payload: { v: 2 } },
    });
    await vi.waitFor(() => expect(lines.length).toBeGreaterThanOrEqual(2), {
      timeout: 1000,
    });
    const responses = lines.slice(0, 2).map((l) => JSON.parse(l).frame);
    expect(responses).toEqual([
      { kind: 'result', id: '1', payload: { v: 1 } },
      { kind: 'result', id: '2', payload: { v: 2 } },
    ]);
    s.destroy();
  });
});
