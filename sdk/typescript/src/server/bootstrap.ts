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
