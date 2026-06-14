//! OSC 5379 and CUP escape-sequence builders.

use crate::error::{OzmaError, OzmaResult};

/// Max inline-webview rows accepted by the VT layer (`1..=MAX_ROWS`).
pub(crate) const MAX_ROWS: u16 = 200;
/// Max inline-webview cols accepted by the VT layer (`1..=MAX_COLS`).
pub(crate) const MAX_COLS: u16 = 400;

/// Returns the `mount-inline` OSC 5379 sequence, or an error if the handle
/// charset is invalid or the dimensions are out of range.
pub(crate) fn mount_inline(handle: &str, rows: u16, cols: u16) -> OzmaResult<String> {
    validate_handle(handle)?;
    if !(1..=MAX_ROWS).contains(&rows) || !(1..=MAX_COLS).contains(&cols) {
        return Err(OzmaError::Register {
            reason: format!("geometry out of range: {rows}x{cols}"),
        });
    }
    Ok(format!(
        "\x1b]5379;mount-inline;{handle};{rows};{cols}\x1b\\"
    ))
}

/// Returns the `unmount-inline` OSC 5379 sequence for a single view handle.
pub(crate) fn unmount_inline(handle: &str) -> String {
    format!("\x1b]5379;unmount-inline;{handle}\x1b\\")
}

/// Returns a CUP (cursor position) sequence for a 0-based viewport cell.
pub(crate) fn cursor_to(row: u16, col: u16) -> String {
    format!("\x1b[{};{}H", row.saturating_add(1), col.saturating_add(1))
}

/// Clamps a (rows, cols) pair into the accepted `1..=MAX` range.
pub(crate) fn clamp_dims(rows: u16, cols: u16) -> (u16, u16) {
    (rows.clamp(1, MAX_ROWS), cols.clamp(1, MAX_COLS))
}

/// Returns whether a view handle matches the `^[A-Za-z0-9._-]{1,128}$` charset.
pub(crate) fn valid_handle(handle: &str) -> bool {
    (1..=128).contains(&handle.len())
        && handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn validate_handle(handle: &str) -> OzmaResult<()> {
    if valid_handle(handle) {
        Ok(())
    } else {
        Err(OzmaError::Register {
            reason: format!("invalid view handle: {handle:?}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_sequence_is_canonical() {
        let s = mount_inline("memo.main", 12, 48).unwrap();
        assert_eq!(s, "\x1b]5379;mount-inline;memo.main;12;48\x1b\\");
    }

    #[test]
    fn unmount_sequence_is_canonical() {
        assert_eq!(
            unmount_inline("memo.main"),
            "\x1b]5379;unmount-inline;memo.main\x1b\\"
        );
    }

    #[test]
    fn cup_is_one_based() {
        assert_eq!(cursor_to(0, 0), "\x1b[1;1H");
        assert_eq!(cursor_to(4, 9), "\x1b[5;10H");
    }

    #[test]
    fn rejects_out_of_range_dims() {
        assert!(mount_inline("h", 0, 10).is_err());
        assert!(mount_inline("h", 201, 10).is_err());
        assert!(mount_inline("h", 10, 401).is_err());
    }

    #[test]
    fn rejects_bad_handle_charset() {
        assert!(mount_inline("bad handle", 10, 10).is_err());
        assert!(mount_inline("", 10, 10).is_err());
    }

    #[test]
    fn clamp_fits_range() {
        assert_eq!(clamp_dims(0, 0), (1, 1));
        assert_eq!(clamp_dims(500, 500), (200, 400));
        assert_eq!(clamp_dims(12, 48), (12, 48));
    }
}
