import * as fs from 'node:fs/promises';
import type { ApiNamespaceMap } from './api-types.ts';
import { bindHostRpcServer } from './rpc-server.ts';

/** Resolved host startup inputs read from the environment. */
export interface HostStartup {
  rpcSockPath: string;
  readyPath: string;
}

/** Reads + validates the host's startup inputs from `env`. Throws naming the first missing var. */
export async function readHostStartup(
  env: Record<string, string | undefined>,
): Promise<HostStartup> {
  const rpcSockPath = env.OZMUX_HOST_RPC_SOCK;
  if (!rpcSockPath) throw new Error('missing env OZMUX_HOST_RPC_SOCK');
  const readyPath = env.OZMUX_HOST_READY_PATH;
  if (!readyPath) throw new Error('missing env OZMUX_HOST_READY_PATH');
  return { rpcSockPath, readyPath };
}

async function main(): Promise<void> {
  const { rpcSockPath, readyPath } = await readHostStartup(process.env);
  // The host boots with an empty API map: every call resolves to `unknown
  // method` until per-webview API registration re-wires this dormant plumbing.
  const api: ApiNamespaceMap = {};
  await bindHostRpcServer(rpcSockPath, api);
  // NOTE: readiness is a FILE written ONLY after the RPC socket is listening, so
  // Rust's existing `<path>/.ready` existence-poll (command.rs) observes a host
  // that is actually ready. Writing it before bind would race the first call.
  await fs.writeFile(readyPath, '');
}

if (import.meta.main) {
  main().catch((err) => {
    console.error('host: fatal', err);
    process.exit(1);
  });
}
