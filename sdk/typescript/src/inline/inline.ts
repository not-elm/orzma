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
  /** Optional client-assigned instance id; same charset/length as a view id. */
  instanceId?: string;
}

function assertId(label: string, value: string): void {
  if (!VIEW_ID_RE.test(value)) {
    throw new RangeError(
      `invalid inline webview ${label} ${JSON.stringify(value)}: must match ${VIEW_ID_RE}`,
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
 * caller positions the cursor first (e.g. print a heading). `view_id` is a
 * handle registered over the control plane (a dynamic registration owned by the
 * writing surface); `rows`/`cols` are validated to 1..=200 / 1..=400
 * (out-of-range throws `RangeError` rather than emitting a sequence the terminal
 * would silently drop). An optional `instanceId` is appended as a 5th field to
 * address a specific instance of the view.
 */
export function mountInline(viewId: string, geometry: InlineGeometry): string {
  assertId('view id', viewId);
  assertDim('rows', geometry.rows, MAX_ROWS);
  assertDim('cols', geometry.cols, MAX_COLS);
  let seq = `${OSC}${OSC_WEBVIEW_CODE};mount-inline;${viewId};${geometry.rows};${geometry.cols}`;
  if (geometry.instanceId !== undefined) {
    assertId('instance id', geometry.instanceId);
    seq += `;${geometry.instanceId}`;
  }
  seq += ST;
  return seq + '\n'.repeat(geometry.rows);
}

/**
 * Builds the `unmount-inline` OSC 5379 sequence. With `viewId` + `instanceId`,
 * unmounts that one instance; with `viewId` only, unmounts every instance of
 * that view; with neither, unmounts every inline webview on the terminal
 * (emitted with no trailing separator — an empty field is malformed). Passing
 * an `instanceId` without a `viewId` throws `RangeError`.
 */
export function unmountInline(viewId?: string, instanceId?: string): string {
  if (viewId === undefined) {
    if (instanceId !== undefined) {
      throw new RangeError('unmountInline: instanceId requires a viewId');
    }
    return `${OSC}${OSC_WEBVIEW_CODE};unmount-inline${ST}`;
  }
  assertId('view id', viewId);
  if (instanceId === undefined) {
    return `${OSC}${OSC_WEBVIEW_CODE};unmount-inline;${viewId}${ST}`;
  }
  assertId('instance id', instanceId);
  return `${OSC}${OSC_WEBVIEW_CODE};unmount-inline;${viewId};${instanceId}${ST}`;
}
