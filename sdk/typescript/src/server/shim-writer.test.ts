import { describe, expect, it } from "vitest";
import { assertCommandName } from "./shim-writer.ts";

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
