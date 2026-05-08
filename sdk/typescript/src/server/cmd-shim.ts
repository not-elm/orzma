import * as net from "node:net";
import type { Writable } from "node:stream";
import type { InvokeFrame, ServerFrame } from "./protocol.ts";
import { encodeFrame } from "./protocol.ts";

export interface BuildInvokeArgs {
  command: string;
  argv: string[];
  cwd: string;
  env: Record<string, string | undefined>;
}

export function buildInvokeFrame(args: BuildInvokeArgs): InvokeFrame {
  const env: Record<string, string> = {};
  for (const key of Object.keys(args.env)) {
    if (!key.startsWith("OZMUX_")) continue;
    const value = args.env[key];
    if (typeof value === "string") env[key] = value;
  }
  return {
    type: "invoke",
    command: args.command,
    argv: args.argv,
    cwd: args.cwd,
    env,
  };
}

export class LineSplitter {
  private buffer = "";

  feed(chunk: Buffer): ServerFrame[] {
    this.buffer += chunk.toString("utf8");
    const parts = this.buffer.split("\n");
    this.buffer = parts.pop() ?? "";
    return parts.filter((s) => s.length > 0).map((line) => JSON.parse(line) as ServerFrame);
  }
}

export interface SignalSource {
  addListener(signal: NodeJS.Signals, handler: () => void): void;
  removeListener(signal: NodeJS.Signals, handler: () => void): void;
}

export interface RunShimArgs {
  socketPath: string;
  command: string;
  argv: string[];
  cwd: string;
  env: Record<string, string | undefined>;
  stdout: Writable;
  stderr: Writable;
  connectTimeoutMs: number;
  signals: SignalSource;
}

export function runShim(args: RunShimArgs): Promise<number> {
  return new Promise((resolve) => {
    const sock = net.connect(args.socketPath);
    const splitter = new LineSplitter();
    let exitCode: number | null = null;

    const onSigint = () => sock.write(encodeFrame({ type: "signal", signal: "SIGINT" }));
    const cleanup = () => args.signals.removeListener("SIGINT", onSigint);

    const timer = setTimeout(() => {
      sock.destroy();
      args.stderr.write(`ozmux: failed to connect to extension socket within ${args.connectTimeoutMs}ms\n`);
      cleanup();
      resolve(127);
    }, args.connectTimeoutMs);

    sock.once("connect", () => {
      clearTimeout(timer);
      args.signals.addListener("SIGINT", onSigint);
      sock.write(encodeFrame(buildInvokeFrame({
        command: args.command,
        argv: args.argv,
        cwd: args.cwd,
        env: args.env,
      })));
    });

    sock.on("data", (chunk: Buffer) => {
      let frames;
      try {
        frames = splitter.feed(chunk);
      } catch (e) {
        args.stderr.write(`ozmux: malformed frame from extension server\n`);
        sock.destroy();
        cleanup();
        resolve(2);
        return;
      }
      for (const f of frames) {
        if (f.type === "stdout") args.stdout.write(Buffer.from(f.data, "base64"));
        else if (f.type === "stderr") args.stderr.write(Buffer.from(f.data, "base64"));
        else if (f.type === "exit") exitCode = f.code;
      }
    });

    sock.on("close", () => {
      clearTimeout(timer);
      cleanup();
      if (exitCode === null) {
        args.stderr.write("ozmux: extension server closed unexpectedly\n");
        resolve(1);
      } else {
        resolve(exitCode);
      }
    });

    sock.on("error", () => { /* close handler resolves */ });
  });
}
