/** Height of the fixed rail, in CSS pixels. */
export const RAIL_HEIGHT = 30;

/** Distance from the viewport top at which scrolled-to headings and anchors stop. */
export const SCROLL_OFFSET = RAIL_HEIGHT + 8;

/** Whether an element whose top edge sits at viewport y `top` has reached the top of the view. */
export function reachedTop(top: number): boolean {
  return top <= SCROLL_OFFSET + 1;
}

/** Distance from the viewport top to the find box, in CSS pixels. */
export const FIND_TOP = RAIL_HEIGHT + 8;

/** Height of the find box, in CSS pixels. */
export const FIND_HEIGHT = 32;

/** Distance from the viewport top below which a match is covered by neither the rail nor the find box. */
export const FIND_CLEARANCE = FIND_TOP + FIND_HEIGHT + 8;

/**
 * Writes the layout values into the `--rail-h`, `--scroll-offset`, `--find-top` and `--find-h`
 * custom properties of `root`.
 */
export function applyLayoutVars(root: HTMLElement): void {
  root.style.setProperty('--rail-h', `${RAIL_HEIGHT}px`);
  root.style.setProperty('--scroll-offset', `${SCROLL_OFFSET}px`);
  root.style.setProperty('--find-top', `${FIND_TOP}px`);
  root.style.setProperty('--find-h', `${FIND_HEIGHT}px`);
}
