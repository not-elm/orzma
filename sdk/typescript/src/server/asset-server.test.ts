import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { afterEach, expect, it } from 'vitest';
import { encodeResponse, serveAssets } from './asset-server.ts';

// Request bytes for path "hi": version=1, u32 len=2, "hi".
const REQ_HI = Buffer.from([1, 0, 0, 0, 2, 0x68, 0x69]);
// Response bytes for {200,"text/html","ok"} — must match protocol.rs RESP_OK.
const RESP_OK = Buffer.from([
  0x00, 0xc8, 0, 0, 0, 9, 0x74, 0x65, 0x78, 0x74, 0x2f, 0x68, 0x74, 0x6d, 0x6c, 0, 0, 0, 2, 0x6f,
  0x6b,
]);

let closer: { close(): void } | undefined;
afterEach(() => closer?.close());

it('encodeResponse matches the cross-language fixture', () => {
  expect(encodeResponse({ status: 200, contentType: 'text/html', body: 'ok' })).toEqual(RESP_OK);
});

it('serves a request over the UDS and round-trips', async () => {
  const sock = path.join(os.tmpdir(), `ozmux-test-${process.pid}-${Date.now()}.sock`);
  closer = serveAssets(
    (p) =>
      p === 'hi'
        ? { status: 200, contentType: 'text/html', body: 'ok' }
        : { status: 404, contentType: 'text/plain', body: 'no' },
    { sockPath: sock },
  );
  await new Promise((r) => setTimeout(r, 50)); // let listen() settle

  const got: Buffer = await new Promise((resolve, reject) => {
    const c = net.connect(sock);
    const chunks: Buffer[] = [];
    c.on('connect', () => c.end(REQ_HI));
    c.on('data', (d) => chunks.push(d));
    c.on('end', () => resolve(Buffer.concat(chunks)));
    c.on('error', reject);
  });
  expect(got).toEqual(RESP_OK);
});
