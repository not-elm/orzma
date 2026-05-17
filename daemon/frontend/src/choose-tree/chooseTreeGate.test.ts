import { afterEach, describe, expect, it } from 'vitest';
import { isChooseTreeOpen, setChooseTreeOpen } from './chooseTreeGate';

afterEach(() => setChooseTreeOpen(false));

describe('chooseTreeGate', () => {
  it('starts closed', () => {
    expect(isChooseTreeOpen()).toBe(false);
  });
  it('reflects setChooseTreeOpen', () => {
    setChooseTreeOpen(true);
    expect(isChooseTreeOpen()).toBe(true);
    setChooseTreeOpen(false);
    expect(isChooseTreeOpen()).toBe(false);
  });
});
