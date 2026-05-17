import { describe, expect, it } from 'vitest';
import { keyToAction } from './treeKeymap';

function ev(opts: Partial<KeyboardEventInit> & { key: string }): KeyboardEvent {
  return new KeyboardEvent('keydown', opts);
}

describe('keyToAction', () => {
  it('maps j and ArrowDown to move down', () => {
    expect(keyToAction(ev({ key: 'j' }))).toEqual({ type: 'move', direction: 'down' });
    expect(keyToAction(ev({ key: 'ArrowDown' }))).toEqual({ type: 'move', direction: 'down' });
  });
  it('maps k and ArrowUp to move up', () => {
    expect(keyToAction(ev({ key: 'k' }))).toEqual({ type: 'move', direction: 'up' });
    expect(keyToAction(ev({ key: 'ArrowUp' }))).toEqual({ type: 'move', direction: 'up' });
  });
  it('maps l and ArrowRight to expand', () => {
    expect(keyToAction(ev({ key: 'l' }))).toEqual({ type: 'expand' });
    expect(keyToAction(ev({ key: 'ArrowRight' }))).toEqual({ type: 'expand' });
  });
  it('maps h and ArrowLeft to collapse', () => {
    expect(keyToAction(ev({ key: 'h' }))).toEqual({ type: 'collapse' });
    expect(keyToAction(ev({ key: 'ArrowLeft' }))).toEqual({ type: 'collapse' });
  });
  it('returns confirm for Enter, cancel for Escape', () => {
    expect(keyToAction(ev({ key: 'Enter' }))).toEqual({ type: 'confirm' });
    expect(keyToAction(ev({ key: 'Escape' }))).toEqual({ type: 'cancel' });
  });
  it('ignores keys with modifiers', () => {
    expect(keyToAction(ev({ key: 'j', ctrlKey: true }))).toBeNull();
    expect(keyToAction(ev({ key: 'l', metaKey: true }))).toBeNull();
    expect(keyToAction(ev({ key: 'Enter', altKey: true }))).toBeNull();
  });
  it('ignores unrecognised keys', () => {
    expect(keyToAction(ev({ key: 'a' }))).toBeNull();
    expect(keyToAction(ev({ key: 'F12' }))).toBeNull();
  });
});
