import { Unpackr } from 'msgpackr/index-no-eval';

/** Foreground/background color. Wire: nil = Default, uint8 = Indexed, [r,g,b] = Rgb. */
export type Color = null | number | [number, number, number];

/** Terminal cursor shape on the wire. */
export type CursorShape = 'block' | 'underline' | 'bar';

/** Cursor state at snapshot time. */
export interface Cursor {
  x: number;
  y: number;
  shape: CursorShape;
  visible: boolean;
}

/** A contiguous run of cells sharing fg/bg/style/hyperlink_id. */
export interface Run {
  cols: number;
  fg: Color;
  bg: Color;
  /** Bitmask: bold=1, italic=2, underline=4, strike=8, reverse=16, dim=32. */
  style: number;
  text: string;
  hyperlink_id: number | null;
}

/** A single dirty row inside a `FrameDelta`. */
export interface DirtyRow {
  row: number;
  runs: Run[];
}

/** A full row of runs (left-to-right) inside a `FrameSnapshot`. */
export interface Row {
  runs: Run[];
}

/** Reason a snapshot was sent (per wire spec). */
export type SnapshotReason = 'initial' | 'reconnect' | 'lagged' | 'resize';

/** Full-screen snapshot frame (msgpack-decoded). */
export interface FrameSnapshot {
  seq: number;
  cols: number;
  rows: number;
  cursor: Cursor;
  rows_data: Row[];
  reason: SnapshotReason;
  modes: string[];
}

/** Differential frame (only dirty rows). */
export interface FrameDelta {
  kind: 'delta';
  seq: number;
  dirty_rows: DirtyRow[];
}

/** Render frame tagged union (matches wire spec § 4 RenderFrame discriminator). */
export type RenderFrame = (FrameSnapshot & { kind?: 'snapshot' }) | FrameDelta;

// Mandatory msgpackr config (mirrors tools/verify-msgpack.ts and Phase 2A wire contract).
// Any drift here is a wire-contract violation.
const unpackr = new Unpackr({
  useRecords: false,
  mapsAsObjects: true,
  int64AsType: 'number',
});

/** Decodes a server-sent binary wire frame into a `RenderFrame`. */
export function decodeFrame(bytes: Uint8Array): RenderFrame {
  return unpackr.unpack(bytes) as RenderFrame;
}
