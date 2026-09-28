import { describe, expect, it } from 'vitest';
import { HeadingTracker } from './headings';
import { SCROLL_OFFSET } from './layout';

const BELOW = SCROLL_OFFSET + 50;

describe('HeadingTracker', () => {
  it('picks the last heading that reached the top of the view', () => {
    const tracker = new HeadingTracker();
    expect(tracker.current([-200, SCROLL_OFFSET, BELOW], 900)).toBe(1);
    expect(tracker.current([BELOW, BELOW + 100], 0)).toBeNull();
  });

  it('keeps a jumped-to heading current while the page stays where the jump left it', () => {
    const tracker = new HeadingTracker();
    tracker.jumped(2, 4311);
    expect(tracker.current([-300, SCROLL_OFFSET, 105, 234], 4311)).toBe(2);
  });

  it('forgets the jump once the page moves', () => {
    const tracker = new HeadingTracker();
    tracker.jumped(2, 4311);
    expect(tracker.current([-300, SCROLL_OFFSET, 105, 234], 4000)).toBe(1);
    expect(tracker.current([-300, SCROLL_OFFSET, 105, 234], 4311)).toBe(1);
  });

  it('ignores a jump to a heading the page no longer has', () => {
    const tracker = new HeadingTracker();
    tracker.jumped(5, 4311);
    expect(tracker.current([SCROLL_OFFSET, BELOW], 4311)).toBe(0);
  });
});
