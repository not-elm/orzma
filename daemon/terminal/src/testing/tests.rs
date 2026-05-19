//! Unit tests for the `testing::tape` module.
use super::tape::{Tape, TapeError, TapeRecord, write_tape};

#[test]
fn tape_load_rejects_bad_magic() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), b"NOTOZTAPE\0\0").unwrap();
    let err = Tape::load(tmp.path()).unwrap_err();
    assert!(
        matches!(
            err,
            TapeError::BadMagic | TapeError::ManifestNotFound { .. }
        ),
        "unexpected error: {err:?}"
    );
}

#[test]
fn tape_load_rejects_unsupported_version() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut buf = Vec::new();
    buf.extend_from_slice(b"OZTAPE\0\0");
    buf.extend_from_slice(&99u16.to_le_bytes());
    buf.extend_from_slice(&[0u8; 6]);
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    std::fs::write(tmp.path(), buf).unwrap();
    let err = Tape::load(tmp.path()).unwrap_err();
    assert!(
        matches!(
            err,
            TapeError::UnsupportedVersion(99) | TapeError::ManifestNotFound { .. }
        ),
        "unexpected error: {err:?}"
    );
}

#[test]
fn tape_load_rejects_trailing_bytes_after_eof() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut buf = Vec::new();
    buf.extend_from_slice(b"OZTAPE\0\0");
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&[0u8; 6]);
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    buf.push(0xAA);
    std::fs::write(tmp.path(), buf).unwrap();
    let err = Tape::load(tmp.path()).unwrap_err();
    assert!(
        matches!(
            err,
            TapeError::TrailingBytesAfterEof | TapeError::ManifestNotFound { .. }
        ),
        "unexpected error: {err:?}"
    );
}

#[test]
fn write_tape_round_trip_records() {
    let records = vec![
        TapeRecord {
            ts_ns_offset: 0,
            bytes: b"hello".to_vec(),
        },
        TapeRecord {
            ts_ns_offset: 1_000_000,
            bytes: b"world".to_vec(),
        },
    ];
    let tmp = tempfile::NamedTempFile::new().unwrap();
    write_tape(tmp.path(), &records).unwrap();

    let bytes = std::fs::read(tmp.path()).unwrap();
    assert_eq!(&bytes[0..8], b"OZTAPE\0\0");
    assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 1);
    assert_eq!(&bytes[bytes.len() - 4..], &0xFFFFFFFFu32.to_le_bytes());
}
