//! PTY tape on-disk format (v1, frozen per spec Section 5.2) + Tape type.
//!
//! A tape is a binary blob of PTY bytes captured from a terminal program.
//! Replayed deterministically into the VT bridge for regression bench inputs.
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"OZTAPE\0\0";
const FORMAT_VERSION: u16 = 1;
const MAX_CHUNK_BYTES: u32 = 4 * 1024 * 1024;
const EOF_SENTINEL: u32 = 0xFFFFFFFF;

/// Errors `Tape::load` and `write_tape` can return.
#[derive(Debug, thiserror::Error)]
pub enum TapeError {
    /// IO failure reading or writing the tape file.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Manifest TOML parse failure.
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    /// File doesn't begin with the OZTAPE magic bytes.
    #[error("bad magic — file is not an OZTAPE tape")]
    BadMagic,
    /// Tape format version is not 1 (the only version this crate handles).
    #[error("unsupported tape version {0} (only {FORMAT_VERSION} supported)")]
    UnsupportedVersion(u16),
    /// Reserved bytes in the header are nonzero.
    #[error("reserved bytes nonzero — file may be corrupt")]
    ReservedNonzero,
    /// A record marker lies in the corrupt range (between MAX_CHUNK_BYTES and EOF_SENTINEL).
    #[error("corrupt chunk_len {0:#x}: outside valid range")]
    Corrupt(u32),
    /// Bytes present after the EOF sentinel (file truncation or appended garbage).
    #[error("trailing bytes after EOF sentinel — file may be corrupt or appended")]
    TrailingBytesAfterEof,
    /// SHA-256 of tape bytes does not match manifest's sha256 field.
    #[error("manifest sha256 mismatch: expected {expected}, computed {actual}")]
    ManifestMismatch { expected: String, actual: String },
    /// Tape captured against a different alacritty_terminal version than the linked one.
    #[error(
        "alacritty_terminal version drift: tape captured against {tape_version}, linked against {linked_version}"
    )]
    AlacrittyVersionDrift {
        tape_version: String,
        linked_version: String,
    },
    /// Manifest TOML sidecar file does not exist next to the tape.
    #[error("manifest sidecar not found at {path:?}")]
    ManifestNotFound { path: PathBuf },
}

/// One PTY write recorded into the tape.
#[derive(Debug, Clone)]
pub struct TapeRecord {
    /// Nanoseconds since tape t=0 (capture-time monotonic clock offset).
    pub ts_ns_offset: u64,
    /// Raw PTY bytes for this write boundary.
    pub bytes: Vec<u8>,
}

/// Loaded PTY tape: records + accompanying manifest metadata.
#[derive(Debug, Clone)]
pub struct Tape {
    /// Sequence of PTY write boundaries.
    pub records: Vec<TapeRecord>,
    /// Sidecar manifest TOML deserialized.
    pub manifest: TapeManifest,
}

/// Sidecar manifest schema (`<tape-name>.manifest.toml`).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TapeManifest {
    /// Hex SHA-256 of the .tape file (verified at load time).
    pub sha256: String,
    /// O(1) capacity-sizing estimator used by `feed_pty_tape`.
    pub estimated_total_wire_messages: usize,
    /// "real" (captured from a live program) or "synthetic" (generated).
    pub created_kind: String,
    /// ISO-8601 capture timestamp.
    pub created_at: String,
    /// Runtime version pins.
    pub runtime: TapeManifestRuntime,
}

/// Version pins that must match the consuming crate.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TapeManifestRuntime {
    /// Version of the `alacritty_terminal` crate active at tape capture time.
    pub alacritty_terminal_version: String,
}

impl Tape {
    /// Loads a tape + its sidecar manifest from disk, verifying integrity.
    ///
    /// Verifies:
    /// 1. Magic bytes + version + reserved-zero in the .tape file header
    /// 2. Each record marker is in valid range (no corrupt files)
    /// 3. EOF sentinel is the last 4 bytes (no trailing garbage)
    /// 4. SHA-256 of tape bytes matches the manifest's `sha256` field
    /// 5. Manifest's `alacritty_terminal_version` matches the linked one
    ///    (best-effort: requires build.rs to have set ALACRITTY_TERMINAL_VERSION;
    ///    skipped if env var was not set at compile time).
    pub fn load(tape_path: &Path) -> Result<Self, TapeError> {
        let bytes = fs::read(tape_path)?;
        let records = parse_records(&bytes)?;

        let manifest_path = manifest_path_for(tape_path);
        if !manifest_path.exists() {
            return Err(TapeError::ManifestNotFound {
                path: manifest_path,
            });
        }
        let manifest_text = fs::read_to_string(&manifest_path)?;
        let manifest: TapeManifest = toml::from_str(&manifest_text)?;

        let actual_sha = hex(Sha256::digest(&bytes).as_slice());
        if actual_sha != manifest.sha256 {
            return Err(TapeError::ManifestMismatch {
                expected: manifest.sha256.clone(),
                actual: actual_sha,
            });
        }

        if let Some(linked_version) = option_env!("ALACRITTY_TERMINAL_VERSION")
            && manifest.runtime.alacritty_terminal_version != linked_version
        {
            return Err(TapeError::AlacrittyVersionDrift {
                tape_version: manifest.runtime.alacritty_terminal_version.clone(),
                linked_version: linked_version.to_string(),
            });
        }

        Ok(Tape { records, manifest })
    }

    /// O(1) capacity sizing for `feed_pty_tape` (read from manifest).
    pub fn estimated_total_wire_messages(&self) -> usize {
        self.manifest.estimated_total_wire_messages
    }
}

/// Writes a tape to disk. Used by `examples/generate_tape.rs` and tests.
pub fn write_tape(path: &Path, records: &[TapeRecord]) -> Result<(), TapeError> {
    let mut buf = Vec::new();
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    buf.extend_from_slice(&[0u8; 6]);
    for r in records {
        let len_u32: u32 = r.bytes.len().try_into().expect("chunk too big");
        if len_u32 > MAX_CHUNK_BYTES {
            return Err(TapeError::Corrupt(len_u32));
        }
        buf.extend_from_slice(&len_u32.to_le_bytes());
        buf.extend_from_slice(&r.ts_ns_offset.to_le_bytes());
        buf.extend_from_slice(&r.bytes);
    }
    buf.extend_from_slice(&EOF_SENTINEL.to_le_bytes());
    fs::write(path, buf)?;
    Ok(())
}

fn manifest_path_for(tape_path: &Path) -> PathBuf {
    let mut p = tape_path.to_path_buf();
    let stem = p.file_stem().map(|s| s.to_owned()).unwrap_or_default();
    p.set_file_name(format!("{}.manifest.toml", stem.to_string_lossy()));
    p
}

fn parse_records(bytes: &[u8]) -> Result<Vec<TapeRecord>, TapeError> {
    if bytes.len() < 16 {
        return Err(TapeError::BadMagic);
    }
    if &bytes[0..8] != MAGIC {
        return Err(TapeError::BadMagic);
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != FORMAT_VERSION {
        return Err(TapeError::UnsupportedVersion(version));
    }
    if bytes[10..16].iter().any(|&b| b != 0) {
        return Err(TapeError::ReservedNonzero);
    }

    let mut out = Vec::new();
    let mut pos = 16;
    loop {
        if pos + 4 > bytes.len() {
            return Err(TapeError::Corrupt(0));
        }
        let marker = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        if marker == EOF_SENTINEL {
            if pos != bytes.len() {
                return Err(TapeError::TrailingBytesAfterEof);
            }
            return Ok(out);
        }
        if marker > MAX_CHUNK_BYTES {
            return Err(TapeError::Corrupt(marker));
        }
        if pos + 8 + marker as usize > bytes.len() {
            return Err(TapeError::Corrupt(marker));
        }
        let ts_ns_offset = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let chunk = bytes[pos..pos + marker as usize].to_vec();
        pos += marker as usize;
        out.push(TapeRecord {
            ts_ns_offset,
            bytes: chunk,
        });
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
