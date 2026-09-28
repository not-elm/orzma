import { describe, expect, it } from 'vitest';
import { applyLayoutVars, FIND_CLEARANCE, RAIL_HEIGHT, reachedTop, SCROLL_OFFSET } from './layout';

describe('layout', () => {
  it('treats a heading stopped at the scroll offset as reached', () => {
    expect(reachedTop(SCROLL_OFFSET)).toBe(true);
    expect(reachedTop(SCROLL_OFFSET + 1)).toBe(true);
    expect(reachedTop(SCROLL_OFFSET + 2)).toBe(false);
  });

  it('keeps the scroll offset below the rail', () => {
    expect(SCROLL_OFFSET).toBe(RAIL_HEIGHT + 8);
  });

  it('writes the layout values as CSS custom properties', () => {
    const root = document.createElement('div');
    applyLayoutVars(root);
    expect(root.style.getPropertyValue('--rail-h')).toBe('30px');
    expect(root.style.getPropertyValue('--scroll-offset')).toBe('38px');
    expect(root.style.getPropertyValue('--find-top')).toBe('38px');
    expect(root.style.getPropertyValue('--find-h')).toBe('32px');
  });

  it('clears the find box with a margin', () => {
    expect(FIND_CLEARANCE).toBe(RAIL_HEIGHT + 48);
  });
});
