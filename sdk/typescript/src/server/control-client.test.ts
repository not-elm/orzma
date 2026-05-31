import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { callControl } from './control-client.ts';

function tmpSock(): string {
  return path.join(
    os.tmpdir(),
    `ozmux-control-test-${process.pid}-${Math.random().toString(36).slice(2)}.sock`,
  );
}

describe('callControl', () => {
  let prev: string | undefined;
  beforeEach(() => {
    prev = process.env.OZMUX_CONTROL_SOCK_PATH;
  });
  afterEach(() => {
    if (prev === undefined) delete process.env.OZMUX_CONTROL_SOCK_PATH;
    else process.env.OZMUX_CONTROL_SOCK_PATH = prev;
  });

  it('sends a call frame and resolves with the result payload', async () => {
    const sock = tmpSock();
    const server = net.createServer((conn) => {
      conn.on('data', (chunk) => {
        const frame = JSON.parse(chunk.toString('utf8').trim());
        expect(frame.kind).toBe('call');
        expect(frame.op).toBe('split');
        expect(frame.pane).toBe('100');
        conn.write(
          `${JSON.stringify({
            kind: 'result',
            id: frame.id,
            payload: { new_pane_id: '7', new_activity_id: '9' },
          })}\n`,
        );
      });
    });
    await new Promise<void>((r) => server.listen(sock, r));
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    const out = await callControl('split', '100', {
      side: 'after',
      orientation: 'vertical',
      activity: { kind: 'extension', html_root: '/x', activity_id: 'test-id' },
    });
    expect(out).toEqual({ new_pane_id: '7', new_activity_id: '9' });
    server.close();
  });

  it('rejects when the host returns an error frame', async () => {
    const sock = tmpSock();
    const server = net.createServer((conn) => {
      conn.on('data', (chunk) => {
        const frame = JSON.parse(chunk.toString('utf8').trim());
        conn.write(
          JSON.stringify({ kind: 'error', id: frame.id, code: 'pane_not_found', message: 'nope' }) +
            '\n',
        );
      });
    });
    await new Promise<void>((r) => server.listen(sock, r));
    process.env.OZMUX_CONTROL_SOCK_PATH = sock;

    await expect(
      callControl('split', '1', {
        side: 'after',
        orientation: 'vertical',
        activity: { kind: 'extension', html_root: '/x', activity_id: 'test-id' },
      }),
    ).rejects.toThrow(/pane_not_found/);
    server.close();
  });

  it('no-ops (resolves with synthetic ids) when the env var is unset', async () => {
    delete process.env.OZMUX_CONTROL_SOCK_PATH;
    const out = await callControl('split', '1', {
      side: 'after',
      orientation: 'vertical',
      activity: { kind: 'extension', html_root: '/x', activity_id: 'test-id' },
    });
    expect(typeof out.new_pane_id).toBe('string');
    expect(typeof out.new_activity_id).toBe('string');
  });
});
