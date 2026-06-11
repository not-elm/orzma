import * as fs from 'node:fs/promises';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { ApiNamespaceMap } from './define-api.ts';
import { bindHostRpcServer } from './rpc-server.ts';

let server: net.Server | undefined;
let sockPath = '';

const api: ApiNamespaceMap = {
  fs: {
    read: async (p: string) => `contents:${p}`,
    size: async (bytes: Uint8Array) => bytes.length,
  },
};

beforeEach(async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'ozmux-host-'));
  sockPath = path.join(dir, 'host.rpc.sock');
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

function rpc(s: net.Socket, frame: unknown): Promise<Record<string, unknown>> {
  return new Promise((resolve) => {
    let buf = '';
    s.on('data', (chunk) => {
      buf += chunk.toString('utf8');
      const idx = buf.indexOf('\n');
      if (idx !== -1) resolve(JSON.parse(buf.slice(0, idx)));
    });
    s.write(`${JSON.stringify(frame)}\n`);
  });
}

describe('bindHostRpcServer', () => {
  it('chmods the socket 0600 and sets maxConnections', async () => {
    server = await bindHostRpcServer(sockPath, api);
    expect((await fs.stat(sockPath)).mode & 0o777).toBe(0o600);
    expect(server.maxConnections).toBe(64);
  });

  it('dispatches a call and returns an ok frame', async () => {
    server = await bindHostRpcServer(sockPath, api);
    const s = await connect();
    const r = await rpc(s, { reqId: '1', ns: 'fs', method: 'read', args: ['/x'] });
    expect(r).toEqual({ reqId: '1', ok: true, value: 'contents:/x' });
    s.destroy();
  });

  it('decodes a {__u8} binary arg before dispatch', async () => {
    server = await bindHostRpcServer(sockPath, api);
    const s = await connect();
    const r = await rpc(s, {
      reqId: '2',
      ns: 'fs',
      method: 'size',
      args: [{ __u8: Buffer.from([1, 2, 3]).toString('base64') }],
    });
    expect(r).toEqual({ reqId: '2', ok: true, value: 3 });
    s.destroy();
  });

  it('returns an error frame for an unknown method', async () => {
    server = await bindHostRpcServer(sockPath, api);
    const s = await connect();
    const r = await rpc(s, { reqId: '3', ns: 'fs', method: 'ghost', args: [] });
    expect(r.ok).toBe(false);
    expect(r.error).toBe('unknown method fs.ghost');
    s.destroy();
  });

  it('returns an error frame for a reqId-addressable but malformed frame (no hang)', async () => {
    server = await bindHostRpcServer(sockPath, api);
    const s = await connect();
    const r = await rpc(s, { reqId: '4', ns: 'fs' }); // missing method/args
    expect(r.reqId).toBe('4');
    expect(r.ok).toBe(false);
    s.destroy();
  });

  it('does not crash on a bare null or non-object JSON line', async () => {
    server = await bindHostRpcServer(sockPath, api);
    const s = await connect();
    s.write('null\n'); // would crash on destructure if unguarded
    s.write('42\n');
    // the server must survive and still answer a valid call on the same socket
    const r = await rpc(s, { reqId: '9', ns: 'fs', method: 'read', args: ['/y'] });
    expect(r).toEqual({ reqId: '9', ok: true, value: 'contents:/y' });
    s.destroy();
  });
});
