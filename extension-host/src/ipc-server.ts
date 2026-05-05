import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { Worker } from "node:worker_threads";
import { z } from "zod";

const StatusMessage = z.object({ type: z.literal("status") });
const RunCommandMessage = z.object({
  type: z.literal("run"),
  command: z.string(),
  argv: z.array(z.string()),
});
const IpcMessage = z.discriminatedUnion("type", [
  StatusMessage,
  RunCommandMessage,
]);
type IpcMessage = z.infer<typeof IpcMessage>;
type RunCommand = z.infer<typeof RunCommandMessage>;

export function launchIpcServer() {
  const socketPath = getIpcPath("ozmux-extension-host");
  removeStaleSocket(socketPath);
  const server = net.createServer((socket) => {
    socket.on("error", (err) => {
      console.error("socket error:", err);
    });

    const rl = readline.createInterface({
      input: socket,
      terminal: false,
      crlfDelay: Infinity,
    });

    rl.on("line", (line) => {
      if (line.length === 0) return;

      let raw: unknown;
      try {
        raw = JSON.parse(line);
      } catch {
        writeLine(socket, { ok: false, error: "invalid_json" });
        return;
      }

      const result = IpcMessage.safeParse(raw);
      if (!result.success) {
        writeLine(socket, {
          ok: false,
          error: "schema",
          issues: result.error.issues,
        });
        return;
      }

      const msg = result.data;
      console.log(msg);
      switch (msg.type) {
        case "status":
          writeLine(socket, { ok: true });
          return;
        case "run":
          startWorker(msg);
          writeLine(socket, { ok: true });
          return;
      }
    });

    rl.on("close", () => {
      console.log("client disconnected");
    });
  });

  server.listen(socketPath, () => {
    console.log(`listening on ${socketPath}`);
  });
}

function startWorker(cmd: RunCommand) {
  const HERE = dirname(fileURLToPath(import.meta.url));
  const BOOTSTRAP_URL = join(HERE, "worker-bootstrap.mjs");
  new Worker(BOOTSTRAP_URL, {
    workerData: {
      scriptPath: join(HERE, "hello.js"),
      command: cmd.command,
    },
    argv: cmd.argv,
  });
}

function writeLine(socket: net.Socket, payload: unknown) {
  socket.write(`${JSON.stringify(payload)}\n`);
}

function getIpcPath(name: string): string {
  if (process.platform === "win32") {
    return `\\\\.\\pipe\\${name}`;
  }

  return path.join(os.tmpdir(), `${name}.sock`);
}

function removeStaleSocket(socketPath: string) {
  if (process.platform === "win32") {
    return;
  }

  try {
    fs.unlinkSync(socketPath);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
      throw err;
    }
  }
}
