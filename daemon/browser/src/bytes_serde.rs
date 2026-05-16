//! Serde helper module: bridges `bytes::Bytes` to `serde_bytes`.
//!
//! Using `#[serde(with = "crate::bytes_serde")]` on a `Bytes` field emits a
//! compact msgpack binary cell (`bin` format) instead of a sequence of
//! integers, which is both smaller and faster to decode.

use bytes::Bytes;
use serde::{Deserializer, Serializer};

pub(crate) fn serialize<S: Serializer>(v: &Bytes, s: S) -> Result<S::Ok, S::Error> {
    serde_bytes::serialize(v.as_ref(), s)
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
    let buf: Vec<u8> = serde_bytes::deserialize(d)?;
    Ok(Bytes::from(buf))
}
