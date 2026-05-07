import { ExtensionHostClient } from "./extension-host-client.ts";

export interface CommandContext {
  argv: string[];
  pane: { sessionId: string; windowId: string; paneId: string };
  env: Record<string, string>;
  cwd: string;
}

export type CommandHandler = (ctx: CommandContext) => Promise<void>;

export async function bootstrap(args: {
  commands: Record<string, CommandHandler>;
  onShutdown?: () => void | Promise<void>;
}) {
  const extensionName = process.env.EXTENSION_NAME;
  if (!extensionName) {
    throw new Error("Missing EXTENSION_NAME");
  }
  const client = await ExtensionHostClient.connect();
  client.on("command-invoke", (m) => {
    const cmd = args.commands[m.command];
    cmd(m.ctx);
  });
  client.on("shutdown", () => {
    args.onShutdown?.();
  });
  client.registerCommands(extensionName, Object.keys(args.commands));
}
