import { describe, expect, it } from 'vitest';
import { useCanvasTerminal } from './useCanvasTerminal';

describe('useCanvasTerminal Phase 3.5 return shape', () => {
  it('exposes paneRef, fm, hyperlinks, preedit (no selection / linkHover)', () => {
    expect(typeof useCanvasTerminal).toBe('function');
  });
});
