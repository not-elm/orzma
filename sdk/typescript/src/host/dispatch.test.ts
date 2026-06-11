import { describe, expect, it } from 'vitest';
import type { ApiNamespaceMap } from './define-api.ts';
import { dispatchHostCall } from './dispatch.ts';

const api: ApiNamespaceMap = {
  fs: {
    read: async (path: string) => `contents:${path}`,
    bytes: async () => new Uint8Array([1, 2]),
    boom: async () => {
      throw new Error('explode');
    },
  },
};

describe('dispatchHostCall', () => {
  it('invokes api[ns][method](...args) and returns an ok frame', async () => {
    const r = await dispatchHostCall(api, { reqId: '1', ns: 'fs', method: 'read', args: ['/x'] });
    expect(r).toEqual({ reqId: '1', ok: true, value: 'contents:/x' });
  });

  it('encodes a binary result as a {__u8} envelope', async () => {
    const r = await dispatchHostCall(api, { reqId: '2', ns: 'fs', method: 'bytes', args: [] });
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toEqual({ __u8: Buffer.from([1, 2]).toString('base64') });
  });

  it('returns an error frame for an unknown namespace', async () => {
    const r = await dispatchHostCall(api, { reqId: '3', ns: 'ghost', method: 'x', args: [] });
    expect(r).toEqual({ reqId: '3', ok: false, error: 'unknown method ghost.x' });
  });

  it('returns an error frame for an unknown method', async () => {
    const r = await dispatchHostCall(api, { reqId: '4', ns: 'fs', method: 'nope', args: [] });
    expect(r).toEqual({ reqId: '4', ok: false, error: 'unknown method fs.nope' });
  });

  it('returns an error frame when the method throws', async () => {
    const r = await dispatchHostCall(api, { reqId: '5', ns: 'fs', method: 'boom', args: [] });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toContain('explode');
  });
});
