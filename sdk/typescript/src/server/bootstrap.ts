import * as fs from "node:fs/promises";
import * as net from "node:net";

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
