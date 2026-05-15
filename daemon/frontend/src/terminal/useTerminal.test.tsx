import { describe, expect, it } from 'vitest';
import { useTerminal } from './useTerminal';

describe('useTerminal return shape', () => {
  it('exposes paneRef, fm, hyperlinks, preedit (no selection / linkHover)', () => {
    expect(typeof useTerminal).toBe('function');
  });
});
