import fs from "node:fs";
import net from "node:net";

const MAX_PATH_LEN = 4096;

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

  const server = net.createServer((socket) => {
    let buf = Buffer.alloc(0);
    socket.on("data", (chunk) => {
      buf = Buffer.concat([buf, chunk]);
      if (buf.length < 5) return;
      if (buf[0] !== 1) return void socket.destroy();
      const pathLen = buf.readUInt32BE(1);
      if (pathLen > MAX_PATH_LEN) return void socket.destroy();
      if (buf.length < 5 + pathLen) return;
      if (buf.length > 5 + pathLen) return void socket.destroy();
      const reqPath = buf.subarray(5, 5 + pathLen).toString("utf8");
      Promise.resolve(handler(reqPath))
        .then((res) => socket.end(encodeResponse(res)))
        .catch(() => socket.destroy());
    });
    socket.on("error", () => socket.destroy());
  });

  server.listen(sockPath);
  const close = () => {
    server.close();
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
