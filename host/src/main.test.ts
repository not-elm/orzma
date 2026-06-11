import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { readHostStartup } from './main.ts';

let dir = '';
beforeEach(async () => {
  dir = await fs.mkdtemp(path.join(os.tmpdir(), 'ozmux-main-'));
});
afterEach(async () => {
  await fs.rm(dir, { recursive: true, force: true });
});

describe('readHostStartup', () => {
  it('reads the rpc sock path, ready path, and parsed manifest from env', async () => {
    const manifestPath = path.join(dir, 'host.json');
    await fs.writeFile(
      manifestPath,
      JSON.stringify({
        extensions: [{ name: 'memo', apiPaths: ['/abs/a.ts'] }],
      }),
    );
    const startup = await readHostStartup({
      OZMUX_HOST_RPC_SOCK: '/tmp/x.sock',
      OZMUX_HOST_READY_PATH: '/tmp/x.ready',
      OZMUX_HOST_MANIFEST: manifestPath,
    });
    expect(startup.rpcSockPath).toBe('/tmp/x.sock');
    expect(startup.readyPath).toBe('/tmp/x.ready');
    expect(startup.manifest.extensions[0].name).toBe('memo');
  });

  it('throws naming each missing required env var', async () => {
    await expect(
      readHostStartup({ OZMUX_HOST_READY_PATH: '/r', OZMUX_HOST_MANIFEST: '/m' }),
    ).rejects.toThrow(/OZMUX_HOST_RPC_SOCK/);
    await expect(
      readHostStartup({ OZMUX_HOST_RPC_SOCK: '/s', OZMUX_HOST_MANIFEST: '/m' }),
    ).rejects.toThrow(/OZMUX_HOST_READY_PATH/);
    await expect(
      readHostStartup({ OZMUX_HOST_RPC_SOCK: '/s', OZMUX_HOST_READY_PATH: '/r' }),
    ).rejects.toThrow(/OZMUX_HOST_MANIFEST/);
  });
});
