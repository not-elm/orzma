import { reachedTop } from './layout';

/** Tracks which heading is being read, from the heading positions and the last jump to a heading. */
export class HeadingTracker {
  private jump: { index: number; scrollY: number } | null = null;

  /** Records a jump to heading `index` that left the page scrolled to document y `scrollY`. */
  jumped(index: number, scrollY: number): void {
    this.jump = { index, scrollY };
  }

  /**
   * Returns the index of the heading being read with the page at document y `scrollY`, given each
   * heading's top in viewport y. While the page stays where the last jump left it, that is the
   * jumped-to heading, even one the page cannot scroll up to the top of the view; otherwise it is
   * the last heading that reached the top of the view, or `null` when none has. Once the page
   * moves, the jump is forgotten.
   */
  current(tops: readonly number[], scrollY: number): number | null {
    const jump = this.jump;
    if (jump !== null && Math.abs(scrollY - jump.scrollY) < 1 && jump.index < tops.length) {
      return jump.index;
    }
    this.jump = null;
    let current: number | null = null;
    for (let i = 0; i < tops.length; i++) {
      if (reachedTop(tops[i])) {
        current = i;
      }
    }
    return current;
  }
}
