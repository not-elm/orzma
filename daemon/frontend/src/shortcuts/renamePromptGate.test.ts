import { afterEach, describe, expect, it } from 'vitest';
import { isRenamePromptOpen, setRenamePromptOpen } from './renamePromptGate';

afterEach(() => {
  setRenamePromptOpen(false);
});

describe('renamePromptGate', () => {
  it('defaults to closed', () => {
    expect(isRenamePromptOpen()).toBe(false);
  });

  it('reflects setRenamePromptOpen', () => {
    setRenamePromptOpen(true);
    expect(isRenamePromptOpen()).toBe(true);
    setRenamePromptOpen(false);
    expect(isRenamePromptOpen()).toBe(false);
  });
});
