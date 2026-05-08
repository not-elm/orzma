import { describe, expect, it } from "vitest";
import { LineSplitter, buildInvokeFrame } from "./cmd-shim.ts";

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

describe("LineSplitter", () => {
  it("yields complete JSON lines and buffers partials", () => {
    const split = new LineSplitter();
    expect(split.feed(Buffer.from('{"type":"stdout","data":"AA=="}\n{"typ'))).toEqual([
      { type: "stdout", data: "AA==" },
    ]);
    expect(split.feed(Buffer.from('e":"exit","code":0}\n'))).toEqual([
      { type: "exit", code: 0 },
    ]);
  });
  it("throws on malformed JSON", () => {
    const split = new LineSplitter();
    expect(() => split.feed(Buffer.from("not-json\n"))).toThrow();
  });
});
