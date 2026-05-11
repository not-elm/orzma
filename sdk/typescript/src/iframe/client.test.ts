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
});

afterEach(() => {
  delete (globalThis as any).WebSocket;
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
