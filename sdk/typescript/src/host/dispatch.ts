import { encodeHostValue } from './binary-codec.ts';
import type { ApiNamespaceMap } from './define-api.ts';

/** A host call as it arrives off the wire (the trusted surface identity lives Rust-side, not here). */
export interface HostCallFrame {
  reqId: string;
  ns: string;
  method: string;
  args: unknown[];
}

/** The dispatcher's reply: success with an (already binary-encoded) value, or an error. */
export type HostResultFrame =
  | { reqId: string; ok: true; value: unknown }
  | { reqId: string; ok: false; error: string };

/**
 * Dispatches a host call to `api[ns][method](...args)`, encoding a binary result
 * via `encodeHostValue`. An unknown namespace/method or a thrown method produces
 * an error frame; this never throws.
 */
export async function dispatchHostCall(
  api: ApiNamespaceMap,
  frame: HostCallFrame,
): Promise<HostResultFrame> {
  const fn = api[frame.ns]?.[frame.method];
  if (typeof fn !== 'function') {
    return { reqId: frame.reqId, ok: false, error: `unknown method ${frame.ns}.${frame.method}` };
  }
  try {
    const value = await (fn as unknown as (...a: unknown[]) => unknown)(...frame.args);
    return { reqId: frame.reqId, ok: true, value: encodeHostValue(value) };
  } catch (e) {
    return { reqId: frame.reqId, ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}
