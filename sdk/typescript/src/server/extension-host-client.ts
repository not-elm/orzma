import * as uds from "node:net";
import type { CommandContext } from "./bootstrap.ts";

const encoder = new TextEncoder();

interface EventMaps {
  "command-invoke": CommandInvoke;
}
type Handler<K extends keyof EventMaps> = (payload: EventMaps[K]) => void;

interface CommandInvoke {
  type: "command-invoke";
  command: string;
  ctx: CommandContext;
}

export class ExtensionHostClient {
  private readonly handlers = new Map<
    keyof EventMaps,
    Set<Handler<keyof EventMaps>>
  >();
  private buffer = "";
  private dataListenerInstalled = false;

  private constructor(private readonly socket: uds.Socket) {}

  async registerCommands(extensionName: string, commands: string[]) {
    this.socket.write(
      encoder.encode(
        JSON.stringify({
          type: "register_commands",
          extensionName,
          commands,
        }) + "\n",
      ),
    );
  }

  on<K extends keyof EventMaps>(eventName: K, f: Handler<K>): void {
    this.ensureDataListener();
    this.addHandler(eventName, f);
  }

  private ensureDataListener(): void {
    if (this.dataListenerInstalled) return;
    this.socket.on("data", (chunk) => this.ingest(chunk));
    this.dataListenerInstalled = true;
  }

  private ingest(chunk: Buffer): void {
    this.buffer += chunk.toString("utf8");
    const lines = this.buffer.split("\n");
    this.buffer = lines.pop() ?? "";
    for (const line of lines) this.dispatchLine(line);
  }

  private dispatchLine(line: string): void {
    if (line.length === 0) return;
    const frame = JSON.parse(line) as CommandInvoke;
    if (frame.type === "command-invoke") {
      this.emit("command-invoke", frame);
    }
  }

  private emit<K extends keyof EventMaps>(
    eventName: K,
    payload: EventMaps[K],
  ): void {
    const set = this.handlers.get(eventName);
    if (!set) return;
    for (const handler of set) (handler as Handler<K>)(payload);
  }

  private addHandler<K extends keyof EventMaps>(
    eventName: K,
    f: Handler<K>,
  ): void {
    let set = this.handlers.get(eventName);
    if (!set) {
      set = new Set();
      this.handlers.set(eventName, set);
    }
    set.add(f as Handler<keyof EventMaps>);
  }

  static async connect(): Promise<ExtensionHostClient> {
    return new Promise((resolve, reject) => {
      const socketPath = process.env.EXTENSION_HOST_SOCKET_PATH;
      if (!socketPath) {
        throw new Error("Missing EXTENSION_HOST_SOCKET_PATH");
      }
      const socket = uds.connect(socketPath);
      socket.once("connect", () => resolve(new ExtensionHostClient(socket)));
      socket.once("error", reject);
    });
  }
}
