import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as channelsServer from "./channels-server.ts";
import { __resetActivityChannelsForTests } from "./channels-server.ts";
import * as daemonClient from "./daemon-client.ts";
import * as handlersServer from "./handlers-server.ts";
import { __resetActivityHandlersForTests } from "./handlers-server.ts";
import { Pane } from "./pane.ts";

let postJsonSpy: ReturnType<typeof vi.spyOn>;
let savedExtensionName: string | undefined;

beforeEach(() => {
  __resetActivityHandlersForTests();
  __resetActivityChannelsForTests();
  postJsonSpy = vi.spyOn(daemonClient, "postJson").mockResolvedValue({});
  // Extension-kind splits/adds depend on EXTENSION_NAME being set, since the
  // SDK forwards it to the daemon as `extension_name` in the activity payload.
  // Each test that exercises that path can rely on this default.
  savedExtensionName = process.env.EXTENSION_NAME;
  process.env.EXTENSION_NAME = "memo";
});

afterEach(() => {
  postJsonSpy.mockRestore();
  if (savedExtensionName === undefined) {
    delete process.env.EXTENSION_NAME;
  } else {
    process.env.EXTENSION_NAME = savedExtensionName;
  }
});

describe("Pane.split", () => {
  it("POSTs to the hierarchical split URL with client-supplied UUIDs", async () => {
    const pane = new Pane({ id: "p1", windowId: "w1", sessionId: "s1" });
    const next = await pane.split({
      side: "after",
      orientation: "horizontal",
      activity: { kind: "terminal" },
    });

    expect(postJsonSpy).toHaveBeenCalledTimes(1);
    const [url, body] = postJsonSpy.mock.calls[0] as [
      string,
      Record<string, unknown>,
    ];
    expect(url).toBe("/windows/w1/panes/p1/split");
    expect(body.side).toBe("after");
    expect(body.orientation).toBe("horizontal");
    expect(typeof body.new_pane_id).toBe("string");
    expect(body.new_pane_id).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
    );
    const activity = body.activity as { activity_id: string; kind: unknown };
    expect(activity.activity_id).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
    );
    expect(activity.kind).toEqual({ type: "terminal" });
    expect(next.id).toBe(body.new_pane_id);
    expect(next.windowId).toBe("w1");
    expect(next.sessionId).toBe("s1");
  });

  it("registers handlers and channels BEFORE the POST resolves (race-free)", async () => {
    const registerHandlersSpy = vi.spyOn(
      handlersServer,
      "registerActivityHandlers",
    );
    const registerChannelsSpy = vi.spyOn(
      channelsServer,
      "registerActivityChannels",
    );
    const callOrder: string[] = [];
    registerHandlersSpy.mockImplementation(() => {
      callOrder.push("register-handlers");
    });
    registerChannelsSpy.mockImplementation(() => {
      callOrder.push("register-channels");
    });
    postJsonSpy.mockImplementation(async () => {
      callOrder.push("post");
      return {};
    });

    const pane = new Pane({ id: "p1", windowId: "w1" });
    await pane.split({
      side: "after",
      orientation: "horizontal",
      activity: {
        kind: "extension",
        html: "/tmp/index.html",
        handlers: { greet: async () => ({}) },
        channels: { tick: async function* () { yield 1; } },
      },
    });

    expect(callOrder).toEqual([
      "register-handlers",
      "register-channels",
      "post",
    ]);
    registerHandlersSpy.mockRestore();
    registerChannelsSpy.mockRestore();
  });

  it("rolls registries back when the POST fails", async () => {
    const unregisterHandlersSpy = vi.spyOn(
      handlersServer,
      "unregisterActivityHandlers",
    );
    const unregisterChannelsSpy = vi.spyOn(
      channelsServer,
      "unregisterActivityChannels",
    );
    postJsonSpy.mockRejectedValueOnce(new Error("boom"));

    const pane = new Pane({ id: "p1", windowId: "w1" });
    await expect(
      pane.split({
        side: "after",
        orientation: "horizontal",
        activity: {
          kind: "extension",
          html: "/tmp/index.html",
          handlers: { greet: async () => ({}) },
          channels: { tick: async function* () { yield 1; } },
        },
      }),
    ).rejects.toThrow("boom");

    expect(unregisterHandlersSpy).toHaveBeenCalledTimes(1);
    expect(unregisterChannelsSpy).toHaveBeenCalledTimes(1);
    // The registered id must equal the unregister target — same activity id.
    expect(unregisterHandlersSpy.mock.calls[0][0]).toBe(
      unregisterChannelsSpy.mock.calls[0][0],
    );
    unregisterHandlersSpy.mockRestore();
    unregisterChannelsSpy.mockRestore();
  });

  it("encodes extension html_root as the parent dir of `html`", async () => {
    const pane = new Pane({ id: "p1", windowId: "w1" });
    await pane.split({
      side: "before",
      orientation: "vertical",
      activity: {
        kind: "extension",
        html: "/opt/memo/index.html",
      },
    });
    const [, body] = postJsonSpy.mock.calls[0] as [
      string,
      Record<string, unknown>,
    ];
    const activity = body.activity as {
      kind: { type: string; html_root: string; extension_name: string };
    };
    expect(activity.kind).toEqual({
      type: "extension",
      html_root: "/opt/memo",
      extension_name: "memo",
    });
  });

  it("forwards EXTENSION_NAME from env as `extension_name` on the activity kind", async () => {
    process.env.EXTENSION_NAME = "diary";
    const pane = new Pane({ id: "p1", windowId: "w1" });
    await pane.split({
      side: "after",
      orientation: "horizontal",
      activity: { kind: "extension", html: "/opt/diary/index.html" },
    });
    const [, body] = postJsonSpy.mock.calls[0] as [
      string,
      Record<string, unknown>,
    ];
    const activity = body.activity as {
      kind: { extension_name: string };
    };
    expect(activity.kind.extension_name).toBe("diary");
  });

  it("throws when EXTENSION_NAME is unset and the activity is extension-kind", async () => {
    delete process.env.EXTENSION_NAME;
    const pane = new Pane({ id: "p1", windowId: "w1" });
    await expect(
      pane.split({
        side: "after",
        orientation: "horizontal",
        activity: { kind: "extension", html: "/opt/memo/index.html" },
      }),
    ).rejects.toThrow(/EXTENSION_NAME/);
  });

  it("does not require EXTENSION_NAME for terminal-kind activities", async () => {
    delete process.env.EXTENSION_NAME;
    const pane = new Pane({ id: "p1", windowId: "w1" });
    await expect(
      pane.split({
        side: "after",
        orientation: "horizontal",
        activity: { kind: "terminal" },
      }),
    ).resolves.toBeDefined();
  });
});

describe("Pane.addActivity", () => {
  it("POSTs to /panes/:pid/activities and returns an Activity handle", async () => {
    const pane = new Pane({ id: "p1", windowId: "w1", sessionId: "s1" });
    const activity = await pane.addActivity({ kind: "terminal" });
    expect(postJsonSpy).toHaveBeenCalledTimes(1);
    const [url] = postJsonSpy.mock.calls[0] as [string, unknown];
    expect(url).toBe("/windows/w1/panes/p1/activities");
    expect(activity.paneId).toBe("p1");
    expect(activity.windowId).toBe("w1");
    expect(activity.sessionId).toBe("s1");
    expect(activity.kind).toEqual({ type: "terminal" });
  });

  it("forwards EXTENSION_NAME on extension-kind activities", async () => {
    const pane = new Pane({ id: "p1", windowId: "w1" });
    await pane.addActivity({
      kind: "extension",
      html: "/opt/memo/index.html",
    });
    const [, body] = postJsonSpy.mock.calls[0] as [
      string,
      Record<string, unknown>,
    ];
    const activity = body.activity as {
      kind: { type: string; html_root: string; extension_name: string };
    };
    expect(activity.kind).toEqual({
      type: "extension",
      html_root: "/opt/memo",
      extension_name: "memo",
    });
  });
});
