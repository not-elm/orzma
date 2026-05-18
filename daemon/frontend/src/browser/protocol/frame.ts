import { Packr, Unpackr } from 'msgpackr/index-no-eval';
import type { BrowserClientMsg, BrowserServerMsg } from './wire';

// NOTE: useRecords: false matches rmp_serde::to_vec_named (named-fields msgpack).
const unpackr = new Unpackr({ useRecords: false, mapsAsObjects: true });
const packr = new Packr({ useRecords: false });

/** Decodes a server-sent binary msgpack frame into a `BrowserServerMsg`. */
export function decode(bytes: ArrayBuffer): BrowserServerMsg {
  return unpackr.unpack(new Uint8Array(bytes)) as BrowserServerMsg;
}

/** Encodes a client message into a msgpack `Uint8Array` for transmission. */
export function encode(msg: BrowserClientMsg): Uint8Array {
  return packr.pack(msg);
}
