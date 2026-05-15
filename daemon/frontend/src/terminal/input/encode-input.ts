import { Packr } from 'msgpackr/index-no-eval';

// NOTE: Matches the Phase 2A wire-contract msgpackr config used in useTerminalSocket.
const packr = new Packr({ useRecords: false, mapsAsObjects: true, int64AsType: 'number' });

/** Wraps raw input bytes in the wire `{kind: "input", data: bytes}` envelope. */
export function encodeInputFrame(bytes: Uint8Array): Uint8Array {
  return packr.pack({ kind: 'input', data: bytes });
}
