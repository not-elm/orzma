import * as fs from 'node:fs/promises';
import { type HostManifest, parseHostManifest } from './descriptors.ts';
import { loadHostApi } from './load.ts';
import { bindHostRpcServer } from './rpc-server.ts';

/** Resolved host startup inputs read from the environment. */
export interface HostStartup {
  rpcSockPath: string;
  readyPath: string;
  manifest: HostManifest;
}

/** Reads + validates the host's startup inputs from `env`. Throws naming the first missing/invalid var. */
export async function readHostStartup(
  env: Record<string, string | undefined>,
): Promise<HostStartup> {
  const rpcSockPath = env.OZMUX_HOST_RPC_SOCK;
  if (!rpcSockPath) throw new Error('missing env OZMUX_HOST_RPC_SOCK');
  const readyPath = env.OZMUX_HOST_READY_PATH;
  if (!readyPath) throw new Error('missing env OZMUX_HOST_READY_PATH');
  const manifestPath = env.OZMUX_HOST_MANIFEST;
  if (!manifestPath) throw new Error('missing env OZMUX_HOST_MANIFEST');
  const manifest = parseHostManifest(await fs.readFile(manifestPath, 'utf8'));
  return { rpcSockPath, readyPath, manifest };
}

async function main(): Promise<void> {
  const { rpcSockPath, readyPath, manifest } = await readHostStartup(process.env);
  const { api, warnings } = await loadHostApi(manifest.plugins, (s) => import(s));
  for (const w of warnings) console.error(`host: ${w}`);
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
