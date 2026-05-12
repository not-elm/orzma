import { afterEach, describe, expect, it, vi } from 'vitest';
import { createKeyDispatcher } from './globalKeyDispatcher';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('createKeyDispatcher', () => {
  it('attachTo registers a capture-phase keydown listener and detachFrom removes it', () => {
    const target = document.createElement('div');
    const addSpy = vi.spyOn(target, 'addEventListener');
    const removeSpy = vi.spyOn(target, 'removeEventListener');
    const dispatcher = createKeyDispatcher(() => {});
    dispatcher.attachTo(target);
    expect(addSpy).toHaveBeenCalledWith('keydown', expect.any(Function), { capture: true });
    dispatcher.detachFrom(target);
    expect(removeSpy).toHaveBeenCalledWith('keydown', expect.any(Function), { capture: true });
  });

  it('invokes the handler when a key is pressed on an attached target', () => {
    const target = document.createElement('div');
    document.body.appendChild(target);
    const handler = vi.fn();
    const dispatcher = createKeyDispatcher(handler);
    dispatcher.attachTo(target);
    target.dispatchEvent(new KeyboardEvent('keydown', { key: 'a', bubbles: true }));
    expect(handler).toHaveBeenCalledTimes(1);
    dispatcher.detachFrom(target);
    target.remove();
  });

  it('attaching to the same target twice is idempotent', () => {
    const target = document.createElement('div');
    const addSpy = vi.spyOn(target, 'addEventListener');
    const dispatcher = createKeyDispatcher(() => {});
    dispatcher.attachTo(target);
    dispatcher.attachTo(target);
    expect(addSpy).toHaveBeenCalledTimes(1);
  });
});
