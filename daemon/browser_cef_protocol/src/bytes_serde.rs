//! serde helper for `bytes::Bytes` → msgpack bin format (avoids u8-array encoding).
//! Pattern adapted from `daemon/browser/src/bytes_serde.rs` (existing in the repo).

use bytes::Bytes;
use serde::{Deserializer, Serializer};

pub fn serialize<S: Serializer>(b: &Bytes, ser: S) -> Result<S::Ok, S::Error> {
    ser.serialize_bytes(b.as_ref())
}

pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Bytes, D::Error> {
    let v: serde_bytes::ByteBuf = serde_bytes::deserialize(de)?;
    Ok(Bytes::from(v.into_vec()))
}
