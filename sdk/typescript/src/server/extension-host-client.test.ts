import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import * as fs from "node:fs/promises";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ExtensionHostClient } from "./extension-host-client.ts";
import type { CommandContext } from "./bootstrap.ts";

interface Harness {
  server: net.Server;
  serverSocket: Promise<net.Socket>;
  sockets: Set<net.Socket>;
  socketPath: string;
}

async function startFakeHost(): Promise<Harness> {
  const socketPath = path.join(
    await fs.mkdtemp(path.join(os.tmpdir(), "ozmux-ext-host-")),
    "host.sock",
  );
  const sockets = new Set<net.Socket>();
  let resolveSocket!: (s: net.Socket) => void;
  const serverSocket = new Promise<net.Socket>((r) => {
    resolveSocket = r;
  });
  const server = net.createServer((s) => {
    sockets.add(s);
    s.on("close", () => sockets.delete(s));
    resolveSocket(s);
  });
  await new Promise<void>((resolve) => server.listen(socketPath, resolve));
  return { server, serverSocket, sockets, socketPath };
}

async function stopFakeHost(h: Harness): Promise<void> {
  for (const s of h.sockets) s.destroy();
  await new Promise<void>((resolve) => h.server.close(() => resolve()));
  await fs.rm(path.dirname(h.socketPath), { recursive: true, force: true });
}

const sampleCtx: CommandContext = {
  argv: ["memo"],
  pane: { sessionId: "s", windowId: "w", paneId: "p" },
  env: {},
  cwd: "/tmp",
};

describe("ExtensionHostClient.on", () => {
  let harness: Harness;
  const originalSocketPath = process.env.EXTENSION_HOST_SOCKET_PATH;

  beforeEach(async () => {
    harness = await startFakeHost();
    process.env.EXTENSION_HOST_SOCKET_PATH = harness.socketPath;
  });

  afterEach(async () => {
    if (originalSocketPath === undefined) {
      delete process.env.EXTENSION_HOST_SOCKET_PATH;
    } else {
      process.env.EXTENSION_HOST_SOCKET_PATH = originalSocketPath;
    }
    await stopFakeHost(harness);
  });

  it("dispatches a single command-invoke frame", async () => {
    const client = await ExtensionHostClient.connect();
    const server = await harness.serverSocket;

    const received: Array<{ type: string; command: string; ctx: CommandContext }> = [];
    const seen = new Promise<void>((resolve) => {
      client.on("command-invoke", (payload) => {
        received.push(payload);
        resolve();
      });
    });

    server.write(
      JSON.stringify({
        type: "command-invoke",
        command: "memo",
        ctx: sampleCtx,
      }) + "\n",
    );

    await seen;
    expect(received).toHaveLength(1);
    expect(received[0]).toEqual({
      type: "command-invoke",
      command: "memo",
      ctx: sampleCtx,
    });
  });

  it("dispatches two frames sent in one chunk", async () => {
    const client = await ExtensionHostClient.connect();
    const server = await harness.serverSocket;

    const received: string[] = [];
    const both = new Promise<void>((resolve) => {
      client.on("command-invoke", ({ command }) => {
        received.push(command);
        if (received.length === 2) resolve();
      });
    });

    const frame1 = JSON.stringify({
      type: "command-invoke",
      command: "a",
      ctx: sampleCtx,
    });
    const frame2 = JSON.stringify({
      type: "command-invoke",
      command: "b",
      ctx: sampleCtx,
    });
    server.write(frame1 + "\n" + frame2 + "\n");

    await both;
    expect(received).toEqual(["a", "b"]);
  });

  it("reassembles a frame split across chunks", async () => {
    const client = await ExtensionHostClient.connect();
    const server = await harness.serverSocket;

    const received: string[] = [];
    const seen = new Promise<void>((resolve) => {
      client.on("command-invoke", ({ command }) => {
        received.push(command);
        resolve();
      });
    });

    const frame =
      JSON.stringify({
        type: "command-invoke",
        command: "split",
        ctx: sampleCtx,
      }) + "\n";
    const half = Math.floor(frame.length / 2);
    server.write(frame.slice(0, half));
    await new Promise((r) => setTimeout(r, 10));
    server.write(frame.slice(half));

    await seen;
    expect(received).toEqual(["split"]);
  });
});
