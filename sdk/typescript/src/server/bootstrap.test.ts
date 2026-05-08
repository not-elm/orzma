import * as fs from "node:fs/promises";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { bindServer, resolveBootstrapEnv } from "./bootstrap.ts";

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

describe("bindServer", () => {
  it("listens on the socket and chmods it 0600", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "ozmux-bind-"));
    const sock = path.join(dir, "x.sock");
    const server = await bindServer(sock, () => {});
    try {
      const stat = await fs.stat(sock);
      expect(stat.mode & 0o777).toBe(0o600);
      await new Promise<void>((res) => {
        const c = net.connect(sock, () => { c.end(); res(); });
      });
    } finally {
      await new Promise<void>((res) => server.close(() => res()));
      await fs.rm(dir, { recursive: true, force: true });
    }
  });

  it("removes a stale socket file before binding", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "ozmux-bind-stale-"));
    const sock = path.join(dir, "x.sock");
    await fs.writeFile(sock, ""); // simulate leftover from previous run
    const server = await bindServer(sock, () => {});
    try {
      const stat = await fs.stat(sock);
      expect(stat.mode & 0o777).toBe(0o600);
    } finally {
      await new Promise<void>((res) => server.close(() => res()));
      await fs.rm(dir, { recursive: true, force: true });
    }
  });
});
