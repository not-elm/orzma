import { describe, expect, it } from "vitest";
import { buildInvokeFrame } from "./cmd-shim.ts";

describe("buildInvokeFrame", () => {
  it("includes argv, cwd, and only OZMUX_* env keys", () => {
    const frame = buildInvokeFrame({
      command: "memo",
      argv: ["a", "b"],
      cwd: "/work",
      env: {
        OZMUX_SESSION_ID: "s",
        OZMUX_WINDOW_ID: "w",
        OZMUX_PANE_ID: "p",
        OZMUX_ACTIVITY_ID: "a",
        PATH: "/bin",
        HOME: "/h",
      },
    });
    expect(frame).toEqual({
      type: "invoke",
      command: "memo",
      argv: ["a", "b"],
      cwd: "/work",
      env: {
        OZMUX_SESSION_ID: "s",
        OZMUX_WINDOW_ID: "w",
        OZMUX_PANE_ID: "p",
        OZMUX_ACTIVITY_ID: "a",
      },
    });
  });
});
