import * as fs from "node:fs/promises";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  bindHandlersServer,
  registerActivityHandlers,
  __resetActivityHandlersForTests,
} from "./handlers-server.ts";
import {
  registerActivityChannels,
  __resetActivityChannelsForTests,
} from "./channels-server.ts";

let server: net.Server | undefined;
let sockPath = "";

beforeEach(async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "ozmux-test-"));
  sockPath = path.join(dir, "chan.handlers.sock");
  __resetActivityHandlersForTests();
  __resetActivityChannelsForTests();
});

afterEach(async () => {
  if (server) {
    await new Promise<void>((res) => server!.close(() => res()));
    server = undefined;
  }
});

function connect(): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const s = net.connect(sockPath);
    s.once("connect", () => resolve(s));
    s.once("error", reject);
  });
}

function lineReader(s: net.Socket): () => Promise<string> {
  const queue: string[] = [];
  const wakers: Array<(line: string) => void> = [];
  let buf = "";
  s.on("data", (chunk) => {
    buf += chunk.toString("utf8");
    while (true) {
      const idx = buf.indexOf("\n");
      if (idx === -1) break;
      const line = buf.slice(0, idx);
      buf = buf.slice(idx + 1);
      const w = wakers.shift();
      if (w) w(line);
      else queue.push(line);
    }
  });
  return () =>
    new Promise<string>((resolve) => {
      const q = queue.shift();
      if (q !== undefined) resolve(q);
      else wakers.push(resolve);
    });
}

describe("channels-server: happy path", () => {
  it("streams sub.data then sub.complete when the generator returns", async () => {
    server = await bindHandlersServer(sockPath);
    registerActivityChannels("aid-1", {
      counter: async function* ({ n }: { n: number }) {
        for (let i = 0; i < n; i++) yield { i };
      },
    });
    const s = await connect();
    const next = lineReader(s);
    s.write(
      JSON.stringify({
        aid: "aid-1",
        frame: { kind: "sub.open", id: "s1", name: "counter", params: { n: 2 } },
      }) + "\n",
    );
    const a = JSON.parse(await next()).frame;
    const b = JSON.parse(await next()).frame;
    const c = JSON.parse(await next()).frame;
    expect(a).toEqual({ kind: "sub.data", id: "s1", payload: { i: 0 } });
    expect(b).toEqual({ kind: "sub.data", id: "s1", payload: { i: 1 } });
    expect(c).toEqual({ kind: "sub.complete", id: "s1" });
    s.destroy();
  });

  it("returns sub.error UNKNOWN_CHANNEL for an unregistered name", async () => {
    server = await bindHandlersServer(sockPath);
    registerActivityChannels("aid-1", {});
    const s = await connect();
    const next = lineReader(s);
    s.write(
      JSON.stringify({
        aid: "aid-1",
        frame: { kind: "sub.open", id: "s9", name: "ghost", params: {} },
      }) + "\n",
    );
    const env = JSON.parse(await next());
    expect(env.frame).toEqual({
      kind: "sub.error",
      id: "s9",
      code: "UNKNOWN_CHANNEL",
      message: "ghost",
    });
    s.destroy();
  });

  it("returns sub.error HANDLER_ERROR when the generator throws", async () => {
    server = await bindHandlersServer(sockPath);
    registerActivityChannels("aid-1", {
      boom: async function* () {
        yield { ok: 1 };
        throw new Error("nope");
      },
    });
    const s = await connect();
    const next = lineReader(s);
    s.write(
      JSON.stringify({
        aid: "aid-1",
        frame: { kind: "sub.open", id: "s2", name: "boom", params: {} },
      }) + "\n",
    );
    const a = JSON.parse(await next()).frame;
    const b = JSON.parse(await next()).frame;
    expect(a).toEqual({ kind: "sub.data", id: "s2", payload: { ok: 1 } });
    expect(b.kind).toBe("sub.error");
    expect(b.code).toBe("HANDLER_ERROR");
    expect(b.message).toContain("nope");
    s.destroy();
  });

  it("ignores call to a channel name and sub.open to a handler name (cross-namespace)", async () => {
    server = await bindHandlersServer(sockPath);
    registerActivityHandlers("aid-1", {
      hi: async () => ({ ok: 1 }),
    });
    registerActivityChannels("aid-1", {
      tick: async function* () {
        yield { t: 1 };
      },
    });
    const s = await connect();
    const next = lineReader(s);
    // sub.open with handler name -> UNKNOWN_CHANNEL
    s.write(
      JSON.stringify({
        aid: "aid-1",
        frame: { kind: "sub.open", id: "x", name: "hi", params: {} },
      }) + "\n",
    );
    const r1 = JSON.parse(await next()).frame;
    expect(r1.kind).toBe("sub.error");
    expect(r1.code).toBe("UNKNOWN_CHANNEL");
    // call with channel name -> UNKNOWN_HANDLER
    s.write(
      JSON.stringify({
        aid: "aid-1",
        frame: { kind: "call", id: "y", name: "tick", payload: {} },
      }) + "\n",
    );
    const r2 = JSON.parse(await next()).frame;
    expect(r2.kind).toBe("error");
    expect(r2.code).toBe("UNKNOWN_HANDLER");
    s.destroy();
  });

  it("supports two concurrent subscriptions on the same connection", async () => {
    server = await bindHandlersServer(sockPath);
    registerActivityChannels("aid-1", {
      a: async function* () {
        yield { from: "a", v: 1 };
        yield { from: "a", v: 2 };
      },
      b: async function* () {
        yield { from: "b", v: 1 };
      },
    });
    const s = await connect();
    const next = lineReader(s);
    s.write(
      JSON.stringify({
        aid: "aid-1",
        frame: { kind: "sub.open", id: "1", name: "a", params: {} },
      }) + "\n",
    );
    s.write(
      JSON.stringify({
        aid: "aid-1",
        frame: { kind: "sub.open", id: "2", name: "b", params: {} },
      }) + "\n",
    );
    const collected: any[] = [];
    for (let i = 0; i < 5; i++) collected.push(JSON.parse(await next()).frame);
    const byId: Record<string, any[]> = {};
    for (const f of collected) {
      (byId[f.id] ??= []).push(f);
    }
    expect(byId["1"]?.[0].payload).toEqual({ from: "a", v: 1 });
    expect(byId["1"]?.[1].payload).toEqual({ from: "a", v: 2 });
    expect(byId["1"]?.[2].kind).toBe("sub.complete");
    expect(byId["2"]?.[0].payload).toEqual({ from: "b", v: 1 });
    expect(byId["2"]?.[1].kind).toBe("sub.complete");
    s.destroy();
  });
});
