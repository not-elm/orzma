import * as fs from 'node:fs/promises';
import * as net from 'node:net';
import type { ApiNamespaceMap } from './api-types.ts';
import { decodeHostValue } from './binary-codec.ts';
import { dispatchHostCall, type HostCallFrame } from './dispatch.ts';

const MAX_RPC_LINE_BYTES = 8 * 1024 * 1024;
const MAX_RPC_RESULT_BYTES = 8 * 1024 * 1024;

/**
 * Binds a Unix-socket NDJSON RPC server that dispatches `{reqId, ns, method, args}`
 * frames against `api`. Binary `{__u8}` args are decoded to `Uint8Array` before
 * dispatch (symmetric with the result-encode path). A `reqId`-addressable but
 * malformed frame gets an error reply (never a silent drop → no caller hang); a
 * wholly-unparseable line (no `reqId`) is dropped. The trusted surface identity
 * and capability check live Rust-side; this server does not re-check them.
 */
export async function bindHostRpcServer(
  sockPath: string,
  api: ApiNamespaceMap,
): Promise<net.Server> {
  await fs.unlink(sockPath).catch(() => {});
  const server = net.createServer(onConnection);
  server.maxConnections = 64;
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(sockPath, () => {
      server.off('error', reject);
      resolve();
    });
  });
  await fs.chmod(sockPath, 0o600);
  return server;

  function onConnection(conn: net.Socket): void {
    let buf = '';
    conn.on('data', (chunk) => {
      buf += chunk.toString('utf8');
      if (buf.length > MAX_RPC_LINE_BYTES) {
        // NOTE: an unframed flood (bytes with no newline) would grow buf without
        // bound; cap it and drop the connection rather than risk OOM on the one
        // host process.
        // TODO: surface a structured error before destroy.
        conn.destroy();
        return;
      }
      let idx = buf.indexOf('\n');
      while (idx !== -1) {
        const line = buf.slice(0, idx);
        buf = buf.slice(idx + 1);
        handleLine(conn, line);
        idx = buf.indexOf('\n');
      }
    });
    conn.on('error', () => {});
  }

  function handleLine(conn: net.Socket, line: string): void {
    let raw: unknown;
    try {
      raw = JSON.parse(line);
    } catch {
      return;
    }
    if (typeof raw !== 'object' || raw === null) {
      // NOTE: a non-object JSON value (null, number, string) carries no
      // addressable reqId; drop it. JSON.parse('null') returns null, which would
      // otherwise throw on the destructure below and crash the host process.
      return;
    }
    const { reqId, ns, method, args } = raw as Partial<HostCallFrame>;
    if (
      typeof reqId !== 'string' ||
      typeof ns !== 'string' ||
      typeof method !== 'string' ||
      !Array.isArray(args)
    ) {
      if (typeof reqId === 'string') {
        conn.write(`${JSON.stringify({ reqId, ok: false, error: 'malformed host call frame' })}\n`);
      }
      return;
    }
    const frame: HostCallFrame = { reqId, ns, method, args: args.map(decodeHostValue) };
    dispatchHostCall(api, frame)
      .then((result) => {
        const resultLine = JSON.stringify(result);
        if (Buffer.byteLength(resultLine, 'utf8') > MAX_RPC_RESULT_BYTES) {
          // NOTE: an oversized result would choke the render process crossing the
          // CEF IPC boundary; reject with an addressable error frame so the
          // caller's reqId Promise settles.
          conn.write(
            `${JSON.stringify({ reqId: frame.reqId, ok: false, error: 'host result exceeds max size' })}\n`,
          );
          return;
        }
        conn.write(`${resultLine}\n`);
      })
      .catch((err) => {
        // NOTE: dispatchHostCall is contracted never to reject; reply with an
        // error frame anyway so a future regression cannot leave the caller's
        // reqId Promise hanging forever.
        console.error('host rpc: dispatch threw', err);
        conn.write(
          `${JSON.stringify({ reqId: frame.reqId, ok: false, error: 'internal host error' })}\n`,
        );
      });
  }
}
