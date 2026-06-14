// src/main.ts
import * as fs2 from "node:fs/promises";

// src/rpc-server.ts
import * as fs from "node:fs/promises";
import * as net from "node:net";

// src/binary-codec.ts
function isBinaryEnvelope(value) {
  return typeof value === "object" && value !== null && typeof value.__u8 === "string";
}
function encodeHostValue(value) {
  if (value instanceof Uint8Array) {
    return { __u8: Buffer.from(value).toString("base64") };
  }
  return value;
}
function decodeHostValue(value) {
  if (isBinaryEnvelope(value)) {
    return new Uint8Array(Buffer.from(value.__u8, "base64"));
  }
  return value;
}

// src/dispatch.ts
async function dispatchHostCall(api, frame) {
  const fn = api[frame.ns]?.[frame.method];
  if (typeof fn !== "function") {
    return { reqId: frame.reqId, ok: false, error: `unknown method ${frame.ns}.${frame.method}` };
  }
  try {
    const value = await fn(...frame.args);
    return { reqId: frame.reqId, ok: true, value: encodeHostValue(value) };
  } catch (e) {
    return { reqId: frame.reqId, ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

// src/rpc-server.ts
var MAX_RPC_LINE_BYTES = 8 * 1024 * 1024;
var MAX_RPC_RESULT_BYTES = 8 * 1024 * 1024;
async function bindHostRpcServer(sockPath, api) {
  await fs.unlink(sockPath).catch(() => {
  });
  const server = net.createServer(onConnection);
  server.maxConnections = 64;
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(sockPath, () => {
      server.off("error", reject);
      resolve();
    });
  });
  await fs.chmod(sockPath, 384);
  return server;
  function onConnection(conn) {
    let buf = "";
    conn.on("data", (chunk) => {
      buf += chunk.toString("utf8");
      if (buf.length > MAX_RPC_LINE_BYTES) {
        conn.destroy();
        return;
      }
      let idx = buf.indexOf("\n");
      while (idx !== -1) {
        const line = buf.slice(0, idx);
        buf = buf.slice(idx + 1);
        handleLine(conn, line);
        idx = buf.indexOf("\n");
      }
    });
    conn.on("error", () => {
    });
  }
  function handleLine(conn, line) {
    let raw;
    try {
      raw = JSON.parse(line);
    } catch {
      return;
    }
    if (typeof raw !== "object" || raw === null) {
      return;
    }
    const { reqId, ns, method, args } = raw;
    if (typeof reqId !== "string" || typeof ns !== "string" || typeof method !== "string" || !Array.isArray(args)) {
      if (typeof reqId === "string") {
        conn.write(`${JSON.stringify({ reqId, ok: false, error: "malformed host call frame" })}
`);
      }
      return;
    }
    const frame = { reqId, ns, method, args: args.map(decodeHostValue) };
    dispatchHostCall(api, frame).then((result) => {
      const resultLine = JSON.stringify(result);
      if (Buffer.byteLength(resultLine, "utf8") > MAX_RPC_RESULT_BYTES) {
        conn.write(
          `${JSON.stringify({ reqId: frame.reqId, ok: false, error: "host result exceeds max size" })}
`
        );
        return;
      }
      conn.write(`${resultLine}
`);
    }).catch((err) => {
      console.error("host rpc: dispatch threw", err);
      conn.write(
        `${JSON.stringify({ reqId: frame.reqId, ok: false, error: "internal host error" })}
`
      );
    });
  }
}

// src/main.ts
async function readHostStartup(env) {
  const rpcSockPath = env.OZMUX_HOST_RPC_SOCK;
  if (!rpcSockPath) throw new Error("missing env OZMUX_HOST_RPC_SOCK");
  const readyPath = env.OZMUX_HOST_READY_PATH;
  if (!readyPath) throw new Error("missing env OZMUX_HOST_READY_PATH");
  return { rpcSockPath, readyPath };
}
async function main() {
  const { rpcSockPath, readyPath } = await readHostStartup(process.env);
  const api = {};
  await bindHostRpcServer(rpcSockPath, api);
  await fs2.writeFile(readyPath, "");
}
if (import.meta.main) {
  main().catch((err) => {
    console.error("host: fatal", err);
    process.exit(1);
  });
}
export {
  readHostStartup
};
