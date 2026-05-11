import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createActivity } from "./activity.ts";
import * as channelsServer from "./channels-server.ts";
import { __resetActivityChannelsForTests } from "./channels-server.ts";
import * as daemonClient from "./daemon-client.ts";
import * as handlersServer from "./handlers-server.ts";
import { __resetActivityHandlersForTests } from "./handlers-server.ts";

let postJsonSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  __resetActivityHandlersForTests();
  __resetActivityChannelsForTests();
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
    const registerSpy = vi.spyOn(handlersServer, "registerActivityHandlers");
    const greet = vi.fn(async ({ name }: { name: string }) => ({
      message: `Hello, ${name}!`,
    }));
    const aid = await createActivity({
      html: "/tmp/x",
      handlers: { greet },
    });
    expect(aid).toBe("aid-42");
    expect(registerSpy).toHaveBeenCalledWith("aid-42", { greet });
    registerSpy.mockRestore();
  });

  it("works without handlers or channels", async () => {
    const registerHandlersSpy = vi.spyOn(
      handlersServer,
      "registerActivityHandlers",
    );
    const registerChannelsSpy = vi.spyOn(
      channelsServer,
      "registerActivityChannels",
    );
    const aid = await createActivity({ html: "/tmp/x" });
    expect(aid).toBe("aid-42");
    expect(registerHandlersSpy).not.toHaveBeenCalled();
    expect(registerChannelsSpy).not.toHaveBeenCalled();
    registerHandlersSpy.mockRestore();
    registerChannelsSpy.mockRestore();
  });

  it("registers channels under the returned aid when provided", async () => {
    const registerSpy = vi.spyOn(channelsServer, "registerActivityChannels");
    const tick = async function* (): AsyncGenerator<{ t: number }> {
      yield { t: 1 };
    };
    const aid = await createActivity({
      html: "/tmp/x",
      channels: { tick },
    });
    expect(aid).toBe("aid-42");
    expect(registerSpy).toHaveBeenCalledWith("aid-42", { tick });
    registerSpy.mockRestore();
  });
});
