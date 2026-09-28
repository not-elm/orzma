/** Height of the fixed rail, in CSS pixels. */
export const RAIL_HEIGHT = 30;

/** Distance from the viewport top at which scrolled-to headings and anchors stop. */
export const SCROLL_OFFSET = RAIL_HEIGHT + 8;

/** Whether an element whose top edge sits at viewport y `top` has reached the top of the view. */
export function reachedTop(top: number): boolean {
  return top <= SCROLL_OFFSET + 1;
}

/** Writes the layout values into the `--rail-h` and `--scroll-offset` custom properties of `root`. */
export function applyLayoutVars(root: HTMLElement): void {
  root.style.setProperty('--rail-h', `${RAIL_HEIGHT}px`);
  root.style.setProperty('--scroll-offset', `${SCROLL_OFFSET}px`);
}
