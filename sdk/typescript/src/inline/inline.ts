/** ECMAScript-erasable only (Node type-stripping): no enum / namespace / param-properties. */

const OSC = '\x1b]';
const ST = '\x1b\\';
const OSC_WEBVIEW_CODE = '5379';
const VIEW_ID_RE = /^[A-Za-z0-9._-]{1,128}$/;
const MAX_ROWS = 200;
const MAX_COLS = 400;

/** Geometry for an inline webview, in terminal cells (spec §2 normative bounds). */
export interface InlineGeometry {
  /** Rect height in cells; integer 1..=200. */
  rows: number;
  /** Rect width in cells; integer 1..=400. */
  cols: number;
}

function assertViewId(viewId: string): void {
  if (!VIEW_ID_RE.test(viewId)) {
    throw new RangeError(
      `invalid inline webview id ${JSON.stringify(viewId)}: must match ${VIEW_ID_RE}`,
    );
  }
}

function assertDim(name: string, value: number, max: number): void {
  if (!Number.isInteger(value) || value < 1 || value > max) {
    throw new RangeError(`inline ${name} must be an integer in 1..=${max}, got ${value}`);
  }
}

/**
 * Builds the `mount-inline` OSC 5379 sequence plus the `rows` newlines that
 * reserve its vertical space, as a single string for one atomic `write()`.
 *
 * The webview is anchored at the cursor position when this is written, so the
 * caller positions the cursor first (e.g. print a heading). `view_id` must be
 * registered in an extension's `ozmux.toml`; `rows`/`cols` are validated to
 * 1..=200 / 1..=400 (out-of-range throws `RangeError` rather than emitting a
 * sequence the terminal would silently drop).
 */
export function mountInline(viewId: string, geometry: InlineGeometry): string {
  assertViewId(viewId);
  assertDim('rows', geometry.rows, MAX_ROWS);
  assertDim('cols', geometry.cols, MAX_COLS);
  const seq = `${OSC}${OSC_WEBVIEW_CODE};mount-inline;${viewId};${geometry.rows};${geometry.cols}${ST}`;
  return seq + '\n'.repeat(geometry.rows);
}

/**
 * Builds the `unmount-inline` OSC 5379 sequence. With a `viewId`, unmounts that
 * inline webview; with none, unmounts every inline webview on the terminal
 * (emitted with no trailing separator — an empty id field is malformed).
 */
export function unmountInline(viewId?: string): string {
  if (viewId !== undefined) {
    assertViewId(viewId);
    return `${OSC}${OSC_WEBVIEW_CODE};unmount-inline;${viewId}${ST}`;
  }
  return `${OSC}${OSC_WEBVIEW_CODE};unmount-inline${ST}`;
}
