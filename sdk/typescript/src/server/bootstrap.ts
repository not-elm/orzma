import * as fs from "node:fs/promises";
import * as net from "node:net";
import * as path from "node:path";
import { Writable } from "node:stream";
import { encodeFrame, MAX_FRAME_PAYLOAD_BYTES, type ClientFrame } from "./protocol.ts";
import { writeShim } from "./shim-writer.ts";

export interface BootstrapEnv {
  binDir: string;
  sockPath: string;
  extensionName: string;
}

export function resolveBootstrapEnv(env: Record<string, string | undefined>): BootstrapEnv {
  const binDir = env.OZMUX_BIN_DIR;
  const sockPath = env.OZMUX_SOCK_PATH;
  const extensionName = env.EXTENSION_NAME;
  for (const [k, v] of Object.entries({ OZMUX_BIN_DIR: binDir, OZMUX_SOCK_PATH: sockPath, EXTENSION_NAME: extensionName })) {
    if (!v) throw new Error(`missing required env: ${k}`);
  }
  return { binDir: binDir!, sockPath: sockPath!, extensionName: extensionName! };
}

export async function bindServer(
  sockPath: string,
  onConnection: (conn: net.Socket) => void,
): Promise<net.Server> {
  await fs.unlink(sockPath).catch(() => {});
  const server = net.createServer(onConnection);
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(sockPath, () => {
      server.off("error", reject);
      resolve();
    });
  });
  await fs.chmod(sockPath, 0o600);
  return server;
}

export interface MaterializeShimsArgs {
  binDir: string;
  sockPath: string;
  commandNames: string[];
  execPath: string;
  helperPath: string;
}

export async function materializeShims(args: MaterializeShimsArgs): Promise<void> {
  await fs.mkdir(args.binDir, { recursive: true, mode: 0o700 });
  await fs.chmod(args.binDir, 0o700);
  for (const name of args.commandNames) {
    await writeShim({
      filePath: path.join(args.binDir, name),
      execPath: args.execPath,
      helperPath: args.helperPath,
      socketPath: args.sockPath,
      commandName: name,
    });
  }
}

export interface CommandContext {
  argv: string[];
  pane: { sessionId: string; windowId: string; paneId: string; activityId: string };
  cwd: string;
  stdout: Writable;
  stderr: Writable;
  signal: AbortSignal;
}

export type CommandHandler = (ctx: CommandContext) => Promise<number | void>;

function chunkWriter(kind: "stdout" | "stderr", target: Writable): Writable {
  return new Writable({
    write(c: Buffer, _enc, cb) {
      let offset = 0;
      while (offset < c.length) {
        const slice = c.subarray(offset, offset + MAX_FRAME_PAYLOAD_BYTES);
        const ok = target.write(encodeFrame({ type: kind, data: slice.toString("base64") }));
        offset += slice.length;
        if (!ok) return target.once("drain", () => cb());
      }
      cb();
    },
  });
}

export async function handleConnection(
  socket: Writable,
  handlers: Record<string, CommandHandler>,
  parseLine: (line: string) => ClientFrame,
  firstLine: string,
): Promise<void> {
  const frame = parseLine(firstLine);
  if (frame.type !== "invoke") {
    socket.write(encodeFrame({ type: "exit", code: 2 }));
    return;
  }
  const handler = handlers[frame.command];
  if (!handler) {
    socket.write(encodeFrame({
      type: "stderr",
      data: Buffer.from(`ozmux: unknown command '${frame.command}'\n`).toString("base64"),
    }));
    socket.write(encodeFrame({ type: "exit", code: 127 }));
    return;
  }
  const stdout = chunkWriter("stdout", socket);
  const stderr = chunkWriter("stderr", socket);
  const ac = new AbortController();
  const ctx: CommandContext = {
    argv: frame.argv,
    pane: {
      sessionId: frame.env.OZMUX_SESSION_ID ?? "",
      windowId: frame.env.OZMUX_WINDOW_ID ?? "",
      paneId: frame.env.OZMUX_PANE_ID ?? "",
      activityId: frame.env.OZMUX_ACTIVITY_ID ?? "",
    },
    cwd: frame.cwd,
    stdout,
    stderr,
    signal: ac.signal,
  };
  let exitCode = 0;
  try {
    const result = await handler(ctx);
    exitCode = typeof result === "number" ? result : 0;
  } catch (err) {
    const stack = err instanceof Error ? err.stack ?? err.message : String(err);
    socket.write(encodeFrame({ type: "stderr", data: Buffer.from(stack + "\n").toString("base64") }));
    exitCode = 1;
  }
  socket.write(encodeFrame({ type: "exit", code: exitCode }));
}
