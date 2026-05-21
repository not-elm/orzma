import { afterEach, beforeEach, describe, expect, it } from "vitest";

beforeEach(() => {
  (globalThis as any).window = {};
});

afterEach(() => {
  delete (globalThis as any).window;
});

describe("getOzmuxContext", () => {
  it("returns the context object from window.ozmux", async () => {
    const { getOzmuxContext } = await import("./client.ts");
    (globalThis as any).window.ozmux = {
      context: {
        sessionId: "s1",
        windowId: "w1",
        paneId: "p1",
        activityId: "a1",
        role: "extension",
      },
      call: async () => undefined,
      subscribe: () => (async function* () {})(),
    };
    const ctx = getOzmuxContext();
    expect(ctx.windowId).toBe("w1");
    expect(ctx.role).toBe("extension");
  });

  it("throws when window.ozmux is absent", async () => {
    const { getOzmuxContext } = await import("./client.ts");
    expect(() => getOzmuxContext()).toThrow(/ozmux/);
  });
});

describe("createClient.call", () => {
  it("forwards arguments to window.ozmux.call and resolves with its result", async () => {
    const { createClient } = await import("./client.ts");
    const seen: Array<{ name: string; payload: unknown }> = [];
    (globalThis as any).window.ozmux = {
      context: {
        sessionId: null,
        windowId: "w1",
        paneId: "p1",
        activityId: "a1",
        role: "extension",
      },
      call: (name: string, payload: unknown) => {
        seen.push({ name, payload });
        return Promise.resolve({ ok: true });
      },
      subscribe: () => (async function* () {})(),
    };
    const c = createClient();
    const r = await c.call("greet", { who: "world" });
    expect(r).toEqual({ ok: true });
    expect(seen).toEqual([{ name: "greet", payload: { who: "world" } }]);
  });

  it("propagates rejections from window.ozmux.call", async () => {
    const { createClient } = await import("./client.ts");
    (globalThis as any).window.ozmux = {
      context: {
        sessionId: null,
        windowId: "w1",
        paneId: "p1",
        activityId: "a1",
        role: "extension",
      },
      call: () => Promise.reject(new Error("boom")),
      subscribe: () => (async function* () {})(),
    };
    const c = createClient();
    await expect(c.call("x", null)).rejects.toThrow("boom");
  });
});

describe("createClient.subscribe", () => {
  it("yields events from window.ozmux.subscribe", async () => {
    const { createClient } = await import("./client.ts");
    async function* gen() {
      yield { v: 1 };
      yield { v: 2 };
    }
    (globalThis as any).window.ozmux = {
      context: {
        sessionId: null,
        windowId: "w1",
        paneId: "p1",
        activityId: "a1",
        role: "extension",
      },
      call: async () => undefined,
      subscribe: () => gen(),
    };
    const c = createClient();
    const out: Array<{ v: number }> = [];
    for await (const ev of c.subscribe<unknown, { v: number }>("count", {})) {
      out.push(ev);
    }
    expect(out).toEqual([{ v: 1 }, { v: 2 }]);
  });

  it("respects AbortSignal abort during iteration", async () => {
    const { createClient } = await import("./client.ts");
    const ac = new AbortController();
    let outerSignal: AbortSignal | undefined;
    async function* gen() {
      try {
        // Stays open until aborted.
        await new Promise<void>((res) => {
          outerSignal?.addEventListener("abort", () => res(), { once: true });
        });
      } finally {
        // emit nothing
      }
    }
    (globalThis as any).window.ozmux = {
      context: {
        sessionId: null,
        windowId: "w1",
        paneId: "p1",
        activityId: "a1",
        role: "extension",
      },
      call: async () => undefined,
      subscribe: (_n: string, _p: unknown, opts?: { signal?: AbortSignal }) => {
        outerSignal = opts?.signal;
        return gen();
      },
    };
    const c = createClient();
    const iter = c.subscribe<unknown, unknown>("hang", {}, { signal: ac.signal });
    const it = iter[Symbol.asyncIterator]();
    const p = it.next();
    ac.abort();
    const r = await p;
    expect(r.done).toBe(true);
  });
});
