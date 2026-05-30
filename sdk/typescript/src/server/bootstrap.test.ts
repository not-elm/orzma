import * as fs from "node:fs/promises";
import * as net from "node:net";
import * as os from "node:os";
import * as path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { Writable } from "node:stream";
import { bindServer, handleConnection, materializeShims, resolveBootstrapEnv, type CommandHandler } from "./bootstrap.ts";

describe("resolveBootstrapEnv", () => {
  it("returns the quintet when all are set including extensionHostUrl", () => {
    expect(resolveBootstrapEnv({
      OZMUX_BIN_DIR: "/b",
      OZMUX_SOCK_PATH: "/s.sock",
      EXTENSION_NAME: "memo",
      OZMUX_EXTENSION_HOST_URL: "http://127.0.0.1:3200",
      OZMUX_HANDLERS_SOCK_PATH: "/h",
    })).toEqual({
      binDir: "/b",
      sockPath: "/s.sock",
      extensionName: "memo",
      extensionHostUrl: "http://127.0.0.1:3200",
      handlersSockPath: "/h",
    });
  });
  it("does not throw when OZMUX_EXTENSION_HOST_URL is absent and returns undefined for extensionHostUrl", () => {
    const result = resolveBootstrapEnv({
      OZMUX_BIN_DIR: "/b",
      OZMUX_SOCK_PATH: "/s.sock",
      EXTENSION_NAME: "memo",
      OZMUX_HANDLERS_SOCK_PATH: "/h",
    });
    expect(result.extensionHostUrl).toBeUndefined();
  });
  it("throws when any required key is missing", () => {
    expect(() => resolveBootstrapEnv({ OZMUX_BIN_DIR: "/b", OZMUX_SOCK_PATH: "/s.sock" }))
      .toThrow(/EXTENSION_NAME/);
  });
  it("requires OZMUX_HANDLERS_SOCK_PATH", () => {
    const env = {
      OZMUX_BIN_DIR: "/b",
      OZMUX_SOCK_PATH: "/s",
      EXTENSION_NAME: "memo",
      // OZMUX_HANDLERS_SOCK_PATH intentionally missing
    };
    expect(() => resolveBootstrapEnv(env)).toThrow(
      /OZMUX_HANDLERS_SOCK_PATH/,
    );
  });
  it("returns handlersSockPath when env is complete", () => {
    const env = {
      OZMUX_BIN_DIR: "/b",
      OZMUX_SOCK_PATH: "/s",
      EXTENSION_NAME: "memo",
      OZMUX_EXTENSION_HOST_URL: "http://x",
      OZMUX_HANDLERS_SOCK_PATH: "/h",
    };
    expect(resolveBootstrapEnv(env)).toEqual({
      binDir: "/b",
      sockPath: "/s",
      extensionName: "memo",
      extensionHostUrl: "http://x",
      handlersSockPath: "/h",
    });
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

describe("materializeShims", () => {
  it("creates the bin dir and one shim per command", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "ozmux-shims-"));
    try {
      const binDir = path.join(dir, "bin");
      await materializeShims({
        binDir,
        sockPath: "/tmp/x.sock",
        commandNames: ["memo", "list"],
        execPath: "/usr/local/bin/node",
        helperPath: "/sdk/cmd-shim.js",
      });
      for (const name of ["memo", "list"]) {
        const p = path.join(binDir, name);
        const stat = await fs.stat(p);
        expect(stat.mode & 0o777).toBe(0o500);
      }
      const dirStat = await fs.stat(binDir);
      expect(dirStat.mode & 0o777).toBe(0o700);
    } finally {
      await fs.rm(dir, { recursive: true, force: true });
    }
  });
});

function clientPair() {
  const chunks: Buffer[] = [];
  const sink = new Writable({
    write(c: Buffer, _e, cb) { chunks.push(c); cb(); },
  });
  return { sink, chunks };
}

describe("handleConnection", () => {
  it("routes invoke to the handler and emits stdout/exit frames", async () => {
    const handlers: Record<string, CommandHandler> = {
      memo: async (ctx) => {
        ctx.stdout.write(Buffer.from("hello"));
        return 0;
      },
    };
    const { sink, chunks } = clientPair();
    await handleConnection(sink as any, handlers, (line) => JSON.parse(line), JSON.stringify({
      type: "invoke",
      command: "memo",
      argv: [],
      cwd: "/tmp",
      env: { OZMUX_SESSION_ID: "s", OZMUX_WINDOW_ID: "w", OZMUX_PANE_ID: "p", OZMUX_ACTIVITY_ID: "a" },
    }));
    const text = Buffer.concat(chunks).toString("utf8");
    expect(text).toMatch(/"type":"stdout"/);
    expect(text).toMatch(/"type":"exit","code":0/);
  });
});

describe("bootstrap()", () => {
  it("materializes shims, binds the socket, and cleans up on SIGTERM", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "ozmux-bs-"));
    const binDir = path.join(dir, "bin");
    const sockPath = path.join(dir, "x.sock");
    const handlersSockPath = path.join(dir, "h.sock");

    const harness = `
      import { bootstrap } from ${JSON.stringify(fileURLToPath(new URL("./bootstrap.ts", import.meta.url)))};
      bootstrap({ commands: { memo: async () => {} } }).catch((e) => { console.error(e); process.exit(11); });
    `;
    const child = spawn(process.execPath, ["--input-type=module", "-e", harness], {
      env: {
        ...process.env,
        OZMUX_BIN_DIR: binDir,
        OZMUX_SOCK_PATH: sockPath,
        EXTENSION_NAME: "memo",
        OZMUX_EXTENSION_HOST_URL: "http://127.0.0.1:3200",
        OZMUX_HANDLERS_SOCK_PATH: handlersSockPath,
      },
      stdio: "inherit",
    });
    try {
      // Wait until shim + sock exist
      const deadline = Date.now() + 3000;
      while (Date.now() < deadline) {
        try {
          await fs.stat(path.join(binDir, "memo"));
          await fs.stat(sockPath);
          await fs.stat(handlersSockPath);  // NEW
          break;
        } catch { await new Promise((r) => setTimeout(r, 50)); }
      }
      await fs.stat(path.join(binDir, "memo"));
      await fs.stat(sockPath);
      await fs.stat(handlersSockPath);  // NEW

      child.kill("SIGTERM");
      await new Promise<void>((res) => child.once("exit", () => res()));

      await expect(fs.stat(path.join(binDir, "memo"))).rejects.toBeTruthy();
      await expect(fs.stat(sockPath)).rejects.toBeTruthy();
      await expect(fs.stat(handlersSockPath)).rejects.toBeTruthy();  // NEW
    } finally {
      try { child.kill("SIGKILL"); } catch {}
      await fs.rm(dir, { recursive: true, force: true });
    }
  }, 10000);
});
