//! Minimal length-prefixed request/response framing over a Unix socket, shared
//! by the host's `fetch` and the Node `serveAssets` helper. All integers are
//! big-endian; one request/response per connection.

use std::io::{Read, Write};

/// Wire-format version byte prefixing every request.
pub const PROTOCOL_VERSION: u8 = 1;
const MAX_PATH_LEN: u32 = 4 * 1024;
const MAX_CTYPE_LEN: u32 = 256;
const MAX_BODY_LEN: u32 = 64 * 1024 * 1024;

/// A request for one asset path (e.g. `"index.html"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Asset path relative to the extension's root; no leading slash.
    pub path: String,
}

/// A served asset: HTTP-like status, MIME type, and body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP-like status (200, 404, 500, …).
    pub status: u16,
    /// MIME type, e.g. `"text/html"`.
    pub content_type: String,
    /// Raw body bytes.
    pub body: Vec<u8>,
}

/// A framing or transport failure while reading/writing the protocol.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// Underlying I/O error.
    #[error("protocol I/O error: {0}")]
    Io(#[source] std::io::Error),
    /// Request version byte did not match [`PROTOCOL_VERSION`].
    #[error("unsupported protocol version: {0}")]
    VersionMismatch(u8),
    /// A length prefix exceeded its cap.
    #[error("frame length exceeds the permitted maximum")]
    TooLarge,
    /// A field's bytes were not valid UTF-8.
    #[error("frame field was not valid UTF-8")]
    Utf8,
    /// The stream ended before a complete frame.
    #[error("stream ended before a complete frame")]
    UnexpectedEof,
}

// NOTE: a hand-written `From` (not thiserror's `#[from]`) because it remaps the
// `UnexpectedEof` io kind to its own variant — `#[from]` would always wrap into
// `Io`, losing that distinction.
impl From<std::io::Error> for ProtocolError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            ProtocolError::UnexpectedEof
        } else {
            ProtocolError::Io(e)
        }
    }
}

/// Writes a request frame: `version`, `u32 path_len`, `path`.
pub fn write_request<W: Write>(w: &mut W, req: &Request) -> Result<(), ProtocolError> {
    w.write_all(&[PROTOCOL_VERSION])?;
    write_bytes(w, req.path.as_bytes())?;
    Ok(())
}

/// Reads a request frame written by [`write_request`].
pub fn read_request<R: Read>(r: &mut R) -> Result<Request, ProtocolError> {
    let mut version = [0u8; 1];
    r.read_exact(&mut version)?;
    if version[0] != PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch(version[0]));
    }
    let path = read_bytes(r, MAX_PATH_LEN)?;
    let path = String::from_utf8(path).map_err(|_| ProtocolError::Utf8)?;
    Ok(Request { path })
}

/// Writes a response frame: `u16 status`, `u32 ctype_len`, ctype, `u32 body_len`, body.
pub fn write_response<W: Write>(w: &mut W, resp: &Response) -> Result<(), ProtocolError> {
    w.write_all(&resp.status.to_be_bytes())?;
    write_bytes(w, resp.content_type.as_bytes())?;
    write_bytes(w, &resp.body)?;
    Ok(())
}

/// Reads a response frame written by [`write_response`].
pub fn read_response<R: Read>(r: &mut R) -> Result<Response, ProtocolError> {
    let mut status = [0u8; 2];
    r.read_exact(&mut status)?;
    let content_type = read_bytes(r, MAX_CTYPE_LEN)?;
    let content_type = String::from_utf8(content_type).map_err(|_| ProtocolError::Utf8)?;
    let body = read_bytes(r, MAX_BODY_LEN)?;
    Ok(Response {
        status: u16::from_be_bytes(status),
        content_type,
        body,
    })
}

fn write_bytes<W: Write>(w: &mut W, bytes: &[u8]) -> Result<(), ProtocolError> {
    let len = u32::try_from(bytes.len()).map_err(|_| ProtocolError::TooLarge)?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(bytes)?;
    Ok(())
}

fn read_bytes<R: Read>(r: &mut R, max: u32) -> Result<Vec<u8>, ProtocolError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len);
    if len > max {
        return Err(ProtocolError::TooLarge);
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: cross-language byte fixture — must match the Node serveAssets encoder exactly.
    // Layout: version=1 | u32 path_len=2 (BE) | "hi"
    const REQ_HI: &[u8] = &[1, 0, 0, 0, 2, b'h', b'i'];
    // NOTE: cross-language byte fixture — must match the Node serveAssets decoder exactly.
    // Layout: u16 status=200 (0x00C8) | u32 ctype_len=9 (BE) | "text/html" | u32 body_len=2 (BE) | "ok"
    const RESP_OK: &[u8] = &[
        0x00, 0xC8, 0, 0, 0, 9, b't', b'e', b'x', b't', b'/', b'h', b't', b'm', b'l', 0, 0, 0, 2,
        b'o', b'k',
    ];

    #[test]
    fn request_encodes_to_fixture() {
        let mut buf = Vec::new();
        write_request(&mut buf, &Request { path: "hi".into() }).unwrap();
        assert_eq!(buf, REQ_HI);
    }

    #[test]
    fn request_round_trips() {
        let mut buf = Vec::new();
        write_request(
            &mut buf,
            &Request {
                path: "index.html".into(),
            },
        )
        .unwrap();
        let got = read_request(&mut buf.as_slice()).unwrap();
        assert_eq!(got.path, "index.html");
    }

    #[test]
    fn response_encodes_to_fixture() {
        let mut buf = Vec::new();
        write_response(
            &mut buf,
            &Response {
                status: 200,
                content_type: "text/html".into(),
                body: b"ok".to_vec(),
            },
        )
        .unwrap();
        assert_eq!(buf, RESP_OK);
    }

    #[test]
    fn response_round_trips() {
        let mut buf = Vec::new();
        let resp = Response {
            status: 404,
            content_type: "text/plain".into(),
            body: b"nope".to_vec(),
        };
        write_response(&mut buf, &resp).unwrap();
        let got = read_response(&mut buf.as_slice()).unwrap();
        assert_eq!(
            (got.status, got.content_type, got.body),
            (404, "text/plain".into(), b"nope".to_vec())
        );
    }

    #[test]
    fn rejects_bad_version() {
        let bytes = [9u8, 0, 0, 0, 1, b'x'];
        assert!(matches!(
            read_request(&mut &bytes[..]),
            Err(ProtocolError::VersionMismatch(9))
        ));
    }

    #[test]
    fn rejects_oversize_length() {
        let bytes = [1u8, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(matches!(
            read_request(&mut &bytes[..]),
            Err(ProtocolError::TooLarge)
        ));
    }

    #[test]
    fn truncated_is_unexpected_eof() {
        let bytes = [1u8, 0, 0, 0, 5, b'h', b'i'];
        assert!(matches!(
            read_request(&mut &bytes[..]),
            Err(ProtocolError::UnexpectedEof)
        ));
    }
}
