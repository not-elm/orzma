import * as net from "node:net";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { Writable } from "node:stream";
import { describe, expect, it } from "vitest";
import { LineSplitter, buildInvokeFrame, runShim } from "./cmd-shim.ts";

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

class CollectStream extends Writable {
  chunks: Buffer[] = [];
  _write(c: Buffer, _enc: string, cb: (err?: Error | null) => void) {
    this.chunks.push(c);
    cb();
  }
  text() { return Buffer.concat(this.chunks).toString("utf8"); }
}

async function bindEcho(sockPath: string, reply: (invoke: any) => string[]): Promise<net.Server> {
  const srv = net.createServer((conn) => {
    let buf = "";
    conn.on("data", (c) => {
      buf += c.toString("utf8");
      const idx = buf.indexOf("\n");
      if (idx === -1) return;
      const invoke = JSON.parse(buf.slice(0, idx));
      buf = buf.slice(idx + 1);
      for (const line of reply(invoke)) conn.write(line + "\n");
      conn.end();
    });
  });
  await new Promise<void>((res) => srv.listen(sockPath, () => res()));
  return srv;
}

describe("runShim", () => {
  it("pipes stdout/stderr base64 payloads and resolves exit code", async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), "ozmux-shim-rt-"));
    const sock = path.join(dir, "x.sock");
    const srv = await bindEcho(sock, () => [
      JSON.stringify({ type: "stdout", data: Buffer.from("hi\n").toString("base64") }),
      JSON.stringify({ type: "stderr", data: Buffer.from("warn\n").toString("base64") }),
      JSON.stringify({ type: "exit", code: 3 }),
    ]);
    try {
      const stdout = new CollectStream();
      const stderr = new CollectStream();
      const code = await runShim({
        socketPath: sock,
        command: "memo",
        argv: ["a"],
        cwd: "/tmp",
        env: { OZMUX_SESSION_ID: "s", OZMUX_WINDOW_ID: "w", OZMUX_PANE_ID: "p", OZMUX_ACTIVITY_ID: "a" },
        stdout, stderr,
        connectTimeoutMs: 1000,
        signals: { addListener() {}, removeListener() {} },
      });
      expect(code).toBe(3);
      expect(stdout.text()).toBe("hi\n");
      expect(stderr.text()).toBe("warn\n");
    } finally {
      await new Promise<void>((res) => srv.close(() => res()));
      await fs.rm(dir, { recursive: true, force: true });
    }
  });
});
