import { describe, expect, it } from 'vitest';
import { installFrames, parentHandOff } from './frames';
import type { KeyHandler } from './keys';

interface FakeWindow extends EventTarget {
  parent: FakeWindow;
  closed: boolean;
  posted: unknown[];
  postMessage(message: unknown, targetOrigin: string): void;
}

function fakeWindow(parent?: FakeWindow): FakeWindow {
  const win = new EventTarget() as FakeWindow;
  win.parent = parent ?? win;
  win.closed = false;
  win.posted = [];
  win.postMessage = (message) => {
    win.posted.push(message);
  };
  return win;
}

function fakeKeys() {
  const calls: unknown[][] = [];
  let enabled = false;
  const keys: KeyHandler = {
    get enabled() {
      return enabled;
    },
    setEnabled(next) {
      calls.push(['setEnabled', next]);
      enabled = next;
    },
    cancelChord: () => calls.push(['cancelChord']),
    blurFocusedInput: () => calls.push(['blurFocusedInput']),
    scrollFromChild: (action, code, timeStamp) =>
      calls.push(['scrollFromChild', action, code, timeStamp]),
    releaseFromChild: (code) => calls.push(['releaseFromChild', code]),
  };
  return { keys, calls };
}

function deliver(to: FakeWindow, data: unknown, source: unknown): Event {
  const event = new Event('message');
  Object.defineProperties(event, { data: { value: data }, source: { value: source } });
  to.dispatchEvent(event);
  return event;
}

const tagged = (message: object) => ({ orzbrowser: message });

describe('installFrames', () => {
  it('says hello to the parent frame, but not from the top frame', () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    installFrames(top, fakeKeys().keys);
    installFrames(child, fakeKeys().keys);
    expect(top.posted).toEqual([tagged({ kind: 'hello' })]);
    expect(child.posted).toEqual([]);
  });

  it("answers a child's hello only while the keys are on", () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    const relay = installFrames(top, fakeKeys().keys);
    deliver(top, tagged({ kind: 'hello' }), child);
    expect(child.posted).toEqual([]);
    relay({ kind: 'keys', enabled: true });
    child.posted = [];
    deliver(top, tagged({ kind: 'hello' }), child);
    expect(child.posted).toEqual([tagged({ kind: 'keys', enabled: true })]);
  });

  it('applies a host message here and passes it to the children that said hello', () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    const silent = fakeWindow(top);
    const { keys, calls } = fakeKeys();
    const relay = installFrames(top, keys);
    deliver(top, tagged({ kind: 'hello' }), child);
    relay({ kind: 'keys', enabled: true });
    relay({ kind: 'cancelChord' });
    relay({ kind: 'blurInput' });
    expect(calls).toEqual([['setEnabled', true], ['cancelChord'], ['blurFocusedInput']]);
    expect(child.posted).toEqual([
      tagged({ kind: 'keys', enabled: true }),
      tagged({ kind: 'cancelChord' }),
      tagged({ kind: 'blurInput' }),
    ]);
    expect(silent.posted).toEqual([]);
  });

  it('passes on no keys message that leaves the keys as they are', () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    const { keys, calls } = fakeKeys();
    const relay = installFrames(top, keys);
    deliver(top, tagged({ kind: 'hello' }), child);
    relay({ kind: 'keys', enabled: false });
    relay({ kind: 'keys', enabled: true });
    relay({ kind: 'keys', enabled: true });
    expect(calls).toEqual([['setEnabled', true]]);
    expect(child.posted).toEqual([tagged({ kind: 'keys', enabled: true })]);
  });

  it("applies the parent's messages and passes them on to its own children", () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    const grandchild = fakeWindow(child);
    const { keys, calls } = fakeKeys();
    installFrames(child, keys);
    deliver(child, tagged({ kind: 'hello' }), grandchild);
    deliver(child, tagged({ kind: 'keys', enabled: true }), top);
    deliver(child, tagged({ kind: 'cancelChord' }), top);
    deliver(child, tagged({ kind: 'blurInput' }), top);
    expect(calls).toEqual([['setEnabled', true], ['cancelChord'], ['blurFocusedInput']]);
    expect(grandchild.posted).toEqual([
      tagged({ kind: 'keys', enabled: true }),
      tagged({ kind: 'cancelChord' }),
      tagged({ kind: 'blurInput' }),
    ]);
  });

  it("runs a child's handed-off scroll and its key release", () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    const { keys, calls } = fakeKeys();
    installFrames(top, keys);
    const event = deliver(top, tagged({ kind: 'scroll', action: 'down', code: 'KeyJ' }), child);
    deliver(top, tagged({ kind: 'release', code: 'KeyJ' }), child);
    expect(calls).toEqual([
      ['scrollFromChild', 'down', 'KeyJ', event.timeStamp],
      ['releaseFromChild', 'KeyJ'],
    ]);
  });

  it('takes messages only from its parent and its children, each in its own direction', () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    const stranger = fakeWindow();
    const { keys, calls } = fakeKeys();
    installFrames(child, keys);
    deliver(child, tagged({ kind: 'keys', enabled: true }), stranger);
    deliver(child, tagged({ kind: 'keys', enabled: true }), child);
    deliver(child, tagged({ kind: 'scroll', action: 'down', code: 'KeyJ' }), top);
    deliver(child, tagged({ kind: 'scroll', action: 'down', code: 'KeyJ' }), stranger);
    expect(calls).toEqual([]);

    const topCalls = fakeKeys();
    installFrames(top, topCalls.keys);
    deliver(top, tagged({ kind: 'keys', enabled: true }), top);
    deliver(top, tagged({ kind: 'keys', enabled: true }), child);
    expect(topCalls.calls).toEqual([]);
  });

  it('ignores a malformed message', () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    const { keys, calls } = fakeKeys();
    installFrames(top, keys);
    deliver(top, tagged({ kind: 'scroll', action: 'sideways', code: 'KeyJ' }), child);
    deliver(top, tagged({ kind: 'scroll', action: 'down' }), child);
    deliver(top, tagged({ kind: 'release' }), child);
    deliver(top, tagged({ kind: 'unknown' }), child);
    deliver(top, { orzbrowser: 'hello' }, child);
    deliver(top, 'orzbrowser', child);
    expect(calls).toEqual([]);

    const childKeys = fakeKeys();
    installFrames(child, childKeys.keys);
    deliver(child, tagged({ kind: 'keys', enabled: 'yes' }), top);
    expect(childKeys.calls).toEqual([]);
  });

  it("hides its messages from the page's listeners and leaves the page's own to it", () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    installFrames(top, fakeKeys().keys);
    const seen: unknown[] = [];
    top.addEventListener('message', (event) => {
      seen.push((event as MessageEvent).data);
    });
    deliver(top, tagged({ kind: 'hello' }), child);
    deliver(top, tagged({ kind: 'bogus' }), child);
    deliver(top, { page: 'data' }, child);
    expect(seen).toEqual([{ page: 'data' }]);
  });

  it('forgets a child whose frame is gone, on a relay or on the next hello', () => {
    const top = fakeWindow();
    const gone = fakeWindow(top);
    const later = fakeWindow(top);
    const relay = installFrames(top, fakeKeys().keys);
    deliver(top, tagged({ kind: 'hello' }), gone);
    gone.closed = true;
    relay({ kind: 'cancelChord' });
    gone.closed = false;
    relay({ kind: 'blurInput' });
    expect(gone.posted).toEqual([]);

    deliver(top, tagged({ kind: 'hello' }), gone);
    gone.closed = true;
    deliver(top, tagged({ kind: 'hello' }), later);
    gone.closed = false;
    relay({ kind: 'cancelChord' });
    expect(gone.posted).toEqual([]);
    expect(later.posted).toEqual([tagged({ kind: 'cancelChord' })]);
  });
});

describe('parentHandOff', () => {
  it('hands scrolls and key releases to the parent frame, and does nothing in the top frame', () => {
    const top = fakeWindow();
    const child = fakeWindow(top);
    const toTop = parentHandOff(child);
    toTop.handOff('down', 'KeyJ');
    toTop.release('KeyJ');
    const fromTop = parentHandOff(top);
    fromTop.handOff('down', 'KeyJ');
    fromTop.release('KeyJ');
    expect(top.posted).toEqual([
      tagged({ kind: 'scroll', action: 'down', code: 'KeyJ' }),
      tagged({ kind: 'release', code: 'KeyJ' }),
    ]);
  });
});
