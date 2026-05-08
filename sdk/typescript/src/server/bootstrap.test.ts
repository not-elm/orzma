import { describe, expect, it } from "vitest";
import { resolveBootstrapEnv } from "./bootstrap.ts";

describe("resolveBootstrapEnv", () => {
  it("returns the trio when all are set", () => {
    expect(resolveBootstrapEnv({
      OZMUX_BIN_DIR: "/b",
      OZMUX_SOCK_PATH: "/s.sock",
      EXTENSION_NAME: "memo",
    })).toEqual({ binDir: "/b", sockPath: "/s.sock", extensionName: "memo" });
  });
  it("throws when any required key is missing", () => {
    expect(() => resolveBootstrapEnv({ OZMUX_BIN_DIR: "/b", OZMUX_SOCK_PATH: "/s.sock" }))
      .toThrow(/EXTENSION_NAME/);
  });
});
