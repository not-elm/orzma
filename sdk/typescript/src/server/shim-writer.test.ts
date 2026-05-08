import { describe, expect, it } from "vitest";
import { assertCommandName } from "./shim-writer.ts";
import { shellSingleQuote } from "./shim-writer.ts";

describe("assertCommandName", () => {
  it("accepts simple lowercase names", () => {
    expect(() => assertCommandName("memo")).not.toThrow();
    expect(() => assertCommandName("foo-bar_2")).not.toThrow();
  });

  it("rejects uppercase, slashes, dots, empty, and overlong names", () => {
    for (const bad of ["", "Foo", "foo/bar", "..", "9start", "a".repeat(65)]) {
      expect(() => assertCommandName(bad), `should reject ${JSON.stringify(bad)}`).toThrow();
    }
  });
});

describe("shellSingleQuote", () => {
  it("wraps plain strings in single quotes", () => {
    expect(shellSingleQuote("/usr/bin/node")).toBe("'/usr/bin/node'");
  });
  it("escapes embedded single quotes with '\\''", () => {
    expect(shellSingleQuote("ab'cd")).toBe("'ab'\\''cd'");
  });
  it("preserves spaces and newlines inside the quotes", () => {
    expect(shellSingleQuote("a b\nc")).toBe("'a b\nc'");
  });
});
