import { describe, expect, it } from 'vitest';
import { installOzmux } from './ozmux-bridge.ts';

function fakeCef() {
  const listeners: Record<string, (raw: unknown) => void> = {};
  const emitted: any[] = [];
  const cef = {
    emit: (payload: unknown) => emitted.push(payload),
    listen: (id: string, cb: (raw: unknown) => void) => { listeners[id] = cb; },
  };
  // deliver a server frame the way HostEmitEvent does: as a JSON STRING
  const deliver = (frame: object) => listeners['ozmux'](JSON.stringify(frame));
  return { cef, emitted, deliver };
}

describe('installOzmux', () => {
  it('call resolves on a matching result frame', async () => {
    const { cef, emitted, deliver } = fakeCef();
    const ozmux = installOzmux(cef as any);
    const p = ozmux.call('greet', { name: 'A' });
    const sent = emitted[0];
    expect(sent.kind).toBe('call');
    expect(sent.name).toBe('greet');
    deliver({ kind: 'result', id: sent.id, payload: { message: 'Hello, A!' } });
    expect(await p).toEqual({ message: 'Hello, A!' });
  });

  it('call rejects on an error frame', async () => {
    const { cef, emitted, deliver } = fakeCef();
    const ozmux = installOzmux(cef as any);
    const p = ozmux.call('nope', {});
    deliver({ kind: 'error', id: emitted[0].id, code: 'UNKNOWN_HANDLER', message: 'nope' });
    await expect(p).rejects.toThrow(/UNKNOWN_HANDLER|nope/);
  });

  it('subscribe yields sub.data then ends on sub.complete', async () => {
    const { cef, emitted, deliver } = fakeCef();
    const ozmux = installOzmux(cef as any);
    const it_ = ozmux.subscribe('clock', { intervalMs: 1 })[Symbol.asyncIterator]();
    const openId = emitted[0].id;
    deliver({ kind: 'sub.data', id: openId, payload: { t: 1 } });
    expect((await it_.next()).value).toEqual({ t: 1 });
    deliver({ kind: 'sub.complete', id: openId });
    expect((await it_.next()).done).toBe(true);
  });
});
