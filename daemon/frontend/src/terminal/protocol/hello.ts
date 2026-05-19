import type { Cursor } from './frame';

/** Server → client hello frame (wire spec § 4.1). */
export interface HelloFrame {
  kind: 'hello';
  seq: 0;
  cols: number;
  rows: number;
  cursor: Cursor;
  escape_caps: string[];
  input_caps: string[];
  bridge_started_at_unix_us?: number;
}

/** Parses a JSON text hello frame. Throws `Error` if the shape is invalid. */
export function parseHello(text: string): HelloFrame {
  const raw = JSON.parse(text) as Partial<HelloFrame> & { kind?: string };
  if (raw.kind !== 'hello') {
    throw new Error(`hello: expected kind="hello", got ${JSON.stringify(raw.kind)}`);
  }
  if (
    typeof raw.cols !== 'number' ||
    typeof raw.rows !== 'number' ||
    typeof raw.cursor !== 'object' ||
    raw.cursor === null ||
    !Array.isArray(raw.escape_caps) ||
    !Array.isArray(raw.input_caps)
  ) {
    throw new Error('hello: missing required fields');
  }
  return raw as HelloFrame;
}
