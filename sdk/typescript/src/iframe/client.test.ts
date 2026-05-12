import { afterEach, beforeEach, describe, expect, it } from "vitest";

type Listener = (ev: { data: string }) => void;

class MockWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  readonly CONNECTING = 0;
  readonly OPEN = 1;
  readonly CLOSING = 2;
  readonly CLOSED = 3;
  static instances: MockWebSocket[] = [];
  url: string;
  readyState = 0;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: Listener | null = null;
  sent: string[] = [];
  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
    queueMicrotask(() => {
      this.readyState = 1;
      this.onopen?.();
    });
  }
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
    this.onclose?.();
  }
  pushFrame(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}

beforeEach(() => {
  MockWebSocket.instances = [];
  (globalThis as any).WebSocket = MockWebSocket;
  (globalThis as any).window = {
    location: { protocol: "http:", host: "localhost:3200", pathname: "/" },
  };
});

afterEach(() => {
  delete (globalThis as any).WebSocket;
  delete (globalThis as any).window;
});

describe("getOzmuxContext", () => {
  it("reads ids from window.__OZMUX__", async () => {
    const { getOzmuxContext } = await import("./client.ts");
    (globalThis as any).window.__OZMUX__ = {
      sessionId: "s1",
      windowId: "w1",
      paneId: "p1",
      activityId: "a1",
    };
    const ctx = getOzmuxContext();
    expect(ctx).toEqual({
      sessionId: "s1",
      windowId: "w1",
      paneId: "p1",
      activityId: "a1",
    });
  });

  it("throws when window.__OZMUX__ is missing", async () => {
    const { getOzmuxContext } = await import("./client.ts");
    expect(() => getOzmuxContext()).toThrow(/__OZMUX__/);
  });
});

describe("createClient hierarchical URL", () => {
  it("builds the hierarchical handlers WS URL from window.__OZMUX__", async () => {
    const { createClient } = await import("./client.ts");
    (globalThis as any).window.__OZMUX__ = {
      sessionId: null,
      windowId: "w1",
      paneId: "p1",
      activityId: "a1",
    };
    createClient();
    const ws = MockWebSocket.instances[0]!;
    expect(ws.url).toBe(
      "ws://localhost:3200/windows/w1/panes/p1/activities/a1/handlers/ws",
    );
  });

  it("falls back to legacy flat URL when activityId is passed explicitly", async () => {
    const { createClient } = await import("./client.ts");
    createClient({ activityId: "a-legacy" });
    const ws = MockWebSocket.instances[0]!;
    expect(ws.url).toBe(
      "ws://localhost:3200/activities/a-legacy/handlers/ws",
    );
  });
});

describe("createClient.call", () => {
  it("sends a call frame and resolves with the result payload", async () => {
    const { createClient } = await import("./client.ts");
    const client = createClient({ activityId: "aid-1", url: "ws://t/x" });
    const p = client.call<{ name: string }, { hi: string }>("greet", {
      name: "x",
    });
    const ws = MockWebSocket.instances[0]!;
    await new Promise<void>((r) => queueMicrotask(() => r()));
    expect(ws.sent).toHaveLength(1);
    const sent = JSON.parse(ws.sent[0]!);
    expect(sent.kind).toBe("call");
    expect(sent.name).toBe("greet");
    expect(sent.payload).toEqual({ name: "x" });
    ws.pushFrame({ kind: "result", id: sent.id, payload: { hi: "yo" } });
    await expect(p).resolves.toEqual({ hi: "yo" });
  });

  it("rejects the call promise on an error frame", async () => {
    const { createClient } = await import("./client.ts");
    const client = createClient({ activityId: "aid-1", url: "ws://t/x" });
    const p = client.call("boom", {});
    const ws = MockWebSocket.instances[0]!;
    await new Promise<void>((r) => queueMicrotask(() => r()));
    const sent = JSON.parse(ws.sent[0]!);
    ws.pushFrame({
      kind: "error",
      id: sent.id,
      code: "HANDLER_ERROR",
      message: "nope",
    });
    await expect(p).rejects.toMatchObject({ message: "nope" });
  });

  it("rejects pending calls when the WS closes", async () => {
    const { createClient } = await import("./client.ts");
    const client = createClient({ activityId: "aid-1", url: "ws://t/x" });
    const p = client.call("greet", {});
    const ws = MockWebSocket.instances[0]!;
    await new Promise<void>((r) => queueMicrotask(() => r()));
    ws.close();
    await expect(p).rejects.toThrow();
  });
});

describe("createClient.subscribe", () => {
  it("yields data frames in order and ends on sub.complete", async () => {
    const { createClient } = await import("./client.ts");
    const client = createClient({ activityId: "aid-1", url: "ws://t/x" });
    const iter = client.subscribe<{ n: number }, { v: number }>("count", {
      n: 3,
    });
    const ws = MockWebSocket.instances[0]!;
    await new Promise<void>((r) => queueMicrotask(() => r()));
    const open = JSON.parse(ws.sent[0]!);
    expect(open.kind).toBe("sub.open");
    expect(open.params).toEqual({ n: 3 });
    const id = open.id;
    ws.pushFrame({ kind: "sub.data", id, payload: { v: 0 } });
    ws.pushFrame({ kind: "sub.data", id, payload: { v: 1 } });
    ws.pushFrame({ kind: "sub.complete", id });
    const collected: { v: number }[] = [];
    for await (const ev of iter) collected.push(ev);
    expect(collected).toEqual([{ v: 0 }, { v: 1 }]);
  });

  it("throws inside for-await when a sub.error arrives", async () => {
    const { createClient } = await import("./client.ts");
    const client = createClient({ activityId: "aid-1", url: "ws://t/x" });
    const iter = client.subscribe("x", {});
    const ws = MockWebSocket.instances[0]!;
    await new Promise<void>((r) => queueMicrotask(() => r()));
    const id = JSON.parse(ws.sent[0]!).id;
    ws.pushFrame({ kind: "sub.data", id, payload: 1 });
    ws.pushFrame({
      kind: "sub.error",
      id,
      code: "HANDLER_ERROR",
      message: "boom",
    });
    let threw: Error | null = null;
    try {
      for await (const ev of iter) void ev;
    } catch (e) {
      threw = e as Error;
    }
    expect(threw?.message).toBe("boom");
  });

  it("sends sub.cancel and stops iteration on AbortSignal.abort()", async () => {
    const { createClient } = await import("./client.ts");
    const client = createClient({ activityId: "aid-1", url: "ws://t/x" });
    const ac = new AbortController();
    const iter = client.subscribe("x", {}, { signal: ac.signal });
    const ws = MockWebSocket.instances[0]!;
    await new Promise<void>((r) => queueMicrotask(() => r()));
    const id = JSON.parse(ws.sent[0]!).id;
    ws.pushFrame({ kind: "sub.data", id, payload: 1 });

    const it = iter[Symbol.asyncIterator]();
    const first = await it.next();
    expect(first.value).toBe(1);
    ac.abort();
    const after = await it.next();
    expect(after.done).toBe(true);

    const cancelFrame = JSON.parse(ws.sent[1]!);
    expect(cancelFrame).toEqual({ kind: "sub.cancel", id });
  });

  it("returns done:true immediately if signal.aborted before subscribe", async () => {
    const { createClient } = await import("./client.ts");
    const client = createClient({ activityId: "aid-1", url: "ws://t/x" });
    const ac = new AbortController();
    ac.abort();
    const iter = client.subscribe("x", {}, { signal: ac.signal });
    const ws = MockWebSocket.instances[0]!;
    await new Promise<void>((r) => queueMicrotask(() => r()));
    expect(ws.sent).toHaveLength(0);
    const it = iter[Symbol.asyncIterator]();
    const r = await it.next();
    expect(r.done).toBe(true);
  });

  it("multiplexes call() and subscribe() without id collision", async () => {
    const { createClient } = await import("./client.ts");
    const client = createClient({ activityId: "aid-1", url: "ws://t/x" });
    const ws = MockWebSocket.instances[0]!;
    await new Promise<void>((r) => queueMicrotask(() => r()));

    const callP = client.call("hi", {});
    const subIter = client.subscribe("s", {});
    await new Promise<void>((r) => queueMicrotask(() => r()));
    const frames = ws.sent.map((s) => JSON.parse(s));
    expect(frames[0].kind).toBe("call");
    expect(frames[1].kind).toBe("sub.open");
    expect(frames[0].id).not.toBe(frames[1].id);

    ws.pushFrame({ kind: "result", id: frames[0].id, payload: "ok" });
    ws.pushFrame({ kind: "sub.complete", id: frames[1].id });
    await expect(callP).resolves.toBe("ok");
    const it = subIter[Symbol.asyncIterator]();
    expect((await it.next()).done).toBe(true);
  });
});
