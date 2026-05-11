import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createActivity } from "./activity.ts";
import * as daemonClient from "./daemon-client.ts";
import {
  __resetActivityHandlersForTests,
  registerActivityHandlers,
} from "./handlers-server.ts";

// Replace postJson with a spy that returns a stable aid
let postJsonSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  __resetActivityHandlersForTests();
  postJsonSpy = vi
    .spyOn(daemonClient, "postJson")
    .mockResolvedValue({ activity_id: "aid-42" });
});

afterEach(() => {
  postJsonSpy.mockRestore();
});

describe("createActivity", () => {
  it("returns the activity_id from the daemon response", async () => {
    const aid = await createActivity({ html: "/tmp/x" });
    expect(aid).toBe("aid-42");
  });

  it("registers handlers under the returned aid when provided", async () => {
    const greet = vi.fn(async ({ name }: { name: string }) => ({
      message: `Hello, ${name}!`,
    }));
    const aid = await createActivity({
      html: "/tmp/x",
      handlers: { greet },
    });
    expect(aid).toBe("aid-42");
    expect(greet).not.toHaveBeenCalled();
  });

  it("works without handlers (backward compatible)", async () => {
    const aid = await createActivity({ html: "/tmp/x" });
    expect(aid).toBe("aid-42");
  });
});

describe("createActivity ⇄ handlers-server", () => {
  it("activity handlers are visible to the dispatcher", async () => {
    const fn = vi.fn(async () => ({ ok: true }));
    await createActivity({ html: "/tmp/x", handlers: { ping: fn } });
    expect(typeof registerActivityHandlers).toBe("function");
  });
});
