/** Wire form of a binary value crossing the JSON-string IPC channel. */
export interface BinaryEnvelope {
  __u8: string;
}

/** Narrows an unknown wire value to a `BinaryEnvelope`. */
export function isBinaryEnvelope(value: unknown): value is BinaryEnvelope {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as { __u8?: unknown }).__u8 === 'string'
  );
}

/**
 * Encodes a host method's return value for the JSON-string channel. A top-level
 * `Uint8Array`/`Buffer` becomes a base64 `BinaryEnvelope`; every other value
 * passes through unchanged. Boundary-tagged: nested binary is NOT walked.
 */
export function encodeHostValue(value: unknown): unknown {
  if (value instanceof Uint8Array) {
    return { __u8: Buffer.from(value).toString('base64') } satisfies BinaryEnvelope;
  }
  return value;
}

/** Reverses `encodeHostValue`: a `BinaryEnvelope` becomes a `Uint8Array`. */
export function decodeHostValue(value: unknown): unknown {
  // NOTE: any value shaped { __u8: string } is treated as binary here, so a host
  // method must never return such a plain object as data — it would be silently
  // decoded to bytes (data corruption). Safe in practice because the only producer
  // of this shape is encodeHostValue (which only emits it for real Uint8Array/Buffer).
  if (isBinaryEnvelope(value)) {
    return new Uint8Array(Buffer.from(value.__u8, 'base64'));
  }
  return value;
}
