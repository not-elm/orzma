import fs from "node:fs";
import net from "node:net";

const MAX_PATH_LEN = 4096;

// NOTE: allowHalfOpen (below) disables Node's auto-close on the peer's FIN, so a
// stalled/partial client would otherwise leak its socket forever and wedge
// server.close(). This idle timeout reaps such sockets. It is well above the
// host client's own fetch timeout, so it never cuts a legitimate slow read.
const SOCKET_IDLE_TIMEOUT_MS = 30_000;

/** A served asset: HTTP-like status, MIME type, and body. */
export interface AssetResponse {
  status: number;
  contentType: string;
  body: Buffer | string;
}

/** Maps a request path to an {@link AssetResponse}; may be async. */
export type AssetHandler = (path: string) => AssetResponse | Promise<AssetResponse>;

/**
 * Serves assets over the ozmux byte protocol on a Unix socket (one
 * request/response per connection). Listens on `opts.sockPath` or
 * `OZMUX_SOCK_PATH`. Mirrors the Rust `protocol.rs` framing.
 */
export function serveAssets(
  handler: AssetHandler,
  opts: { sockPath?: string } = {},
): { close(): void } {
  const sockPath = opts.sockPath ?? process.env.OZMUX_SOCK_PATH;
  if (!sockPath) throw new Error("serveAssets: OZMUX_SOCK_PATH not set");
  try {
    fs.unlinkSync(sockPath);
  } catch {
    // no stale socket to remove
  }

  const sockets = new Set<net.Socket>();

  // NOTE: allowHalfOpen keeps the writable side open after the client
  // half-closes (the host's fetch shuts down its write end once the
  // length-prefixed request is sent). Without it, Node auto-ends the socket on
  // the client FIN and the async file-read response is dropped before it sends.
  const server = net.createServer({ allowHalfOpen: true }, (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    socket.setTimeout(SOCKET_IDLE_TIMEOUT_MS, () => socket.destroy());
    let buf = Buffer.alloc(0);
    let consumed = false;
    socket.on("data", (chunk) => {
      if (consumed) return;
      buf = Buffer.concat([buf, chunk]);
      if (buf.length < 5) return;
      if (buf[0] !== 1) return void socket.destroy();
      const pathLen = buf.readUInt32BE(1);
      if (pathLen > MAX_PATH_LEN) return void socket.destroy();
      if (buf.length < 5 + pathLen) return;
      if (buf.length > 5 + pathLen) return void socket.destroy();
      consumed = true;
      const reqPath = buf.subarray(5, 5 + pathLen).toString("utf8");
      Promise.resolve(handler(reqPath))
        .then((res) => {
          if (!socket.destroyed) socket.end(encodeResponse(res));
        })
        .catch(() => socket.destroy());
    });
    socket.on("error", () => socket.destroy());
  });

  server.listen(sockPath);
  const close = () => {
    server.close();
    // Destroy lingering sockets so server.close() can complete: allowHalfOpen
    // means a half-open peer would otherwise keep the server from closing.
    for (const socket of sockets) socket.destroy();
    sockets.clear();
    for (const sig of ["SIGINT", "SIGTERM"] as const) process.off(sig, close);
    try {
      fs.unlinkSync(sockPath);
    } catch {
      // already gone
    }
  };
  for (const sig of ["SIGINT", "SIGTERM"] as const) process.on(sig, close);
  return { close };
}

/** Encodes an {@link AssetResponse} into a protocol response frame. */
export function encodeResponse(r: AssetResponse): Buffer {
  const ctype = Buffer.from(r.contentType, "utf8");
  const body = Buffer.isBuffer(r.body) ? r.body : Buffer.from(r.body, "utf8");
  const head = Buffer.alloc(2 + 4 + ctype.length + 4);
  head.writeUInt16BE(r.status, 0);
  head.writeUInt32BE(ctype.length, 2);
  ctype.copy(head, 6);
  head.writeUInt32BE(body.length, 6 + ctype.length);
  return Buffer.concat([head, body]);
}
