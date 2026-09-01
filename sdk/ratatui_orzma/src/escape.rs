//! APC webview verbs and CUP escape-sequence builders.

use crate::error::{OrzmaError, OrzmaResult};

/// Max webview rows accepted by the VT layer (`1..=MAX_ROWS`).
pub(crate) const MAX_ROWS: u16 = 200;
/// Max webview cols accepted by the VT layer (`1..=MAX_COLS`).
pub(crate) const MAX_COLS: u16 = 400;

/// Returns the `mount` APC verb, or an error if the instance id is not in
/// its minted form or the dimensions are out of range.
pub(crate) fn mount(instance: &str, rows: u16, cols: u16) -> OrzmaResult<String> {
    validate_instance(instance)?;
    if !(1..=MAX_ROWS).contains(&rows) || !(1..=MAX_COLS).contains(&cols) {
        return Err(OrzmaError::Register {
            reason: format!("geometry out of range: {rows}x{cols}"),
        });
    }
    Ok(format!("\x1b_Omount;n={instance},r={rows},c={cols}\x1b\\"))
}

/// Returns the `unmount` APC verb for a single placement.
pub(crate) fn unmount(instance: &str) -> String {
    format!("\x1b_Ounmount;n={instance}\x1b\\")
}

/// Returns a CUP (cursor position) sequence for a 0-based viewport cell.
pub(crate) fn cursor_to(row: u16, col: u16) -> String {
    format!("\x1b[{};{}H", row.saturating_add(1), col.saturating_add(1))
}

/// Clamps a (rows, cols) pair into the accepted `1..=MAX` range.
pub(crate) fn clamp_dims(rows: u16, cols: u16) -> (u16, u16) {
    (rows.clamp(1, MAX_ROWS), cols.clamp(1, MAX_COLS))
}

/// Returns whether a value is exactly 32 lowercase hex digits — the wire
/// form of a control-plane-minted instance id.
pub(crate) fn valid_instance(instance: &str) -> bool {
    instance.len() == 32
        && instance
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn validate_instance(instance: &str) -> OrzmaResult<()> {
    if valid_instance(instance) {
        Ok(())
    } else {
        Err(OrzmaError::Instance {
            reason: format!("not a minted instance id: {instance:?}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "3f5a9c02d1e84b7690ab3cde12f45678";

    /// Asserts that a mount renders as the instance-addressed APC verb.
    ///
    /// Case: a companion app reserves a 12x48 region for a placement the
    /// control plane minted for it.
    #[test]
    fn mount_sequence_is_canonical() {
        let s = mount(ID, 12, 48).unwrap();
        assert_eq!(s, format!("\x1b_Omount;n={ID},r=12,c=48\x1b\\"));
    }

    /// Asserts that an unmount names only the instance.
    ///
    /// Case: a companion app tears one of its placements down.
    #[test]
    fn unmount_sequence_is_canonical() {
        assert_eq!(unmount(ID), format!("\x1b_Ounmount;n={ID}\x1b\\"));
    }

    #[test]
    fn cup_is_one_based() {
        assert_eq!(cursor_to(0, 0), "\x1b[1;1H");
        assert_eq!(cursor_to(4, 9), "\x1b[5;10H");
    }

    /// Asserts that dimensions outside the accepted range are rejected
    /// even when the instance is well-formed, so the range check is
    /// reachable.
    ///
    /// Case: a layout collapses to zero rows before a draw.
    #[test]
    fn rejects_out_of_range_dims() {
        assert!(mount(ID, 0, 10).is_err());
        assert!(mount(ID, 201, 10).is_err());
        assert!(mount(ID, 10, 401).is_err());
    }

    /// Asserts that only the exact 32-lowercase-hex form is accepted, so a
    /// value that is not a minted instance cannot reach the wire.
    ///
    /// Case: a caller hand-writes an id, or forwards one that arrived from
    /// somewhere other than `instance_id()`.
    #[test]
    fn rejects_anything_but_the_canonical_instance_form() {
        assert!(!valid_instance("nf2k7q5w3x3m5a6b2c4d6e7f"));
        assert!(!valid_instance(&ID.to_uppercase()));
        assert!(!valid_instance(&ID[..31]));
        assert!(valid_instance(ID));
    }

    #[test]
    fn clamp_fits_range() {
        assert_eq!(clamp_dims(0, 0), (1, 1));
        assert_eq!(clamp_dims(500, 500), (200, 400));
        assert_eq!(clamp_dims(12, 48), (12, 48));
    }
}
