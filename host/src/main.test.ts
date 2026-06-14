import { describe, expect, it } from 'vitest';
import { readHostStartup } from './main.ts';

describe('readHostStartup', () => {
  it('reads the rpc sock path and ready path from env', async () => {
    const startup = await readHostStartup({
      OZMUX_HOST_RPC_SOCK: '/tmp/x.sock',
      OZMUX_HOST_READY_PATH: '/tmp/x.ready',
    });
    expect(startup.rpcSockPath).toBe('/tmp/x.sock');
    expect(startup.readyPath).toBe('/tmp/x.ready');
  });

  it('throws naming each missing required env var', async () => {
    await expect(readHostStartup({ OZMUX_HOST_READY_PATH: '/r' })).rejects.toThrow(
      /OZMUX_HOST_RPC_SOCK/,
    );
    await expect(readHostStartup({ OZMUX_HOST_RPC_SOCK: '/s' })).rejects.toThrow(
      /OZMUX_HOST_READY_PATH/,
    );
  });
});
