import type { InvokeFrame } from "./protocol.ts";

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
