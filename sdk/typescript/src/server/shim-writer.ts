const COMMAND_NAME_RE = /^[a-z][a-z0-9_-]{0,63}$/;

export function assertCommandName(name: string): void {
  if (!COMMAND_NAME_RE.test(name)) {
    throw new Error(`invalid command name: ${JSON.stringify(name)}`);
  }
}

export function shellSingleQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

import * as fs from "node:fs/promises";

export interface WriteShimArgs {
  filePath: string;
  execPath: string;
  helperPath: string;
  socketPath: string;
  commandName: string;
}

export async function writeShim(args: WriteShimArgs): Promise<void> {
  assertCommandName(args.commandName);
  const lines = [
    "#!/bin/sh",
    `exec ${shellSingleQuote(args.execPath)} ${shellSingleQuote(args.helperPath)} \\`,
    `     --socket ${shellSingleQuote(args.socketPath)} \\`,
    `     --command ${shellSingleQuote(args.commandName)} \\`,
    `     -- "$@"`,
    "",
  ];
  await fs.writeFile(args.filePath, lines.join("\n"), { mode: 0o500 });
  await fs.chmod(args.filePath, 0o500);
}
