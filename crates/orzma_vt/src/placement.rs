//! Webview placement vocabulary: the id a mount is addressed by, the
//! rectangle it reserves, the grid-space geometry an emitted frame
//! carries, and the per-terminal cap.
//!
//! The table itself belongs to each `Screen`; see
//! [`crate::screen::placements`].

use crate::screen::grid::coords::GridPoint;
use std::fmt;
use std::str::FromStr;

/// Host-minted identity of one webview placement.
///
/// The control plane mints it and hands it to the registering program
/// before that program writes its mount, so the program can address the
/// placement it is about to create. The VT never mints one.
///
/// # Invariants
///
/// The wire spelling is exactly 32 lowercase hex digits, so a value and
/// its spelling are in bijection. At any instant the live ids on one
/// terminal are unique: `supersede` drops the existing entry for an id
/// across both screens before a re-mount registers its successor.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(pub u128);

/// The reason a wire spelling is not a valid [`InstanceId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceIdParseError;

impl FromStr for InstanceId {
    type Err = InstanceIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // NOTE: the length and charset are checked here rather than left
        // to `from_str_radix`, which also accepts uppercase digits and a
        // leading `+`; either would give one placement two spellings.
        if s.len() != Self::WIRE_DIGITS
            || !s
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(InstanceIdParseError);
        }
        u128::from_str_radix(s, 16)
            .map(Self)
            .map_err(|_| InstanceIdParseError)
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: the width comes from WIRE_DIGITS so the parser and the
        // renderer cannot drift apart.
        write!(f, "{:0width$x}", self.0, width = Self::WIRE_DIGITS)
    }
}

impl fmt::Debug for InstanceId {
    // NOTE: derived Debug would print decimal, so every tracing line and
    // every signal dump would disagree with the 32-hex wire spelling.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InstanceId({self})")
    }
}

impl InstanceId {
    /// Digits in this id's wire spelling.
    pub const WIRE_DIGITS: usize = 32;

    /// Builds an id from 16 bytes of caller-supplied entropy.
    ///
    /// The randomness stays with the caller: the control plane mints
    /// ids, and putting a self-seeding constructor here would leave a
    /// way for the VT to start minting again.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(u128::from_be_bytes(bytes))
    }
}

/// One placement's grid-space geometry at emit time.
///
/// The point is in active-grid coordinates and does not move when the
/// user scrolls, the same way a cursor point or a selection endpoint
/// does not; the consumer projects it with the frame's display offset.
///
/// # Invariants
///
/// `size` is the reservation the most recent mount for `id` made. A
/// re-mount of a live id keeps the id and may change the size, so the
/// consumer re-reads `size` from every frame rather than caching it
/// against the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchoredPlacement {
    /// The placement this geometry belongs to.
    pub id: InstanceId,
    /// Active-grid cell the rect's top-left corner sits at.
    pub point: GridPoint,
    /// The rect's extent in cells.
    pub size: PlacementSize,
}

/// The cell rectangle a mount reserves, without its position.
///
/// This is deliberately not [`GridSize`](crate::prelude::GridSize), whose row count is the source
/// of truth for one screenful; a placement's reservation is a sub-rectangle
/// and must not be substitutable for a grid dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementSize {
    /// Reserved height in cells.
    pub rows: u16,
    /// Reserved width in cells.
    pub cols: u16,
}

/// Upper bound on live placements per terminal, across both screens.
///
/// It matches the renderer's overlay slot count, so a mount the VT
/// accepts is always one the host can place. The two are not mirrors:
/// the host allocates slots per terminal among live children, while this
/// cap counts both screens, so it is strictly the stricter of the two.
pub const MAX_PLACEMENTS: usize = 12;

/// Upper bound on a mount's reserved rows. With the ~2:1 terminal cell
/// aspect and DPR 2, a 200-row x 400-col mount is a near-square pixel
/// region staying under the common 8192 px GPU texture dimension limit.
pub const MAX_ROWS: u16 = 200;

/// Upper bound on a mount's reserved cols; see [`MAX_ROWS`] for the sizing
/// envelope.
pub const MAX_COLS: u16 = 400;

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a wire spelling parses only as exactly 32 lowercase
    /// hex digits, rejecting the shorter, longer, uppercase, and
    /// sign-prefixed forms `u128::from_str_radix` would otherwise accept.
    ///
    /// Case: a program writes an instance id into a mount APC, and the
    /// terminal must decide whether the field is well-formed before it
    /// reserves a rectangle for it.
    #[test]
    fn an_instance_id_parses_only_from_32_lowercase_hex_digits() {
        let canonical = "3f5a9c02d1e84b7690ab3cde12f45678";
        assert_eq!(
            canonical
                .parse::<InstanceId>()
                .expect("canonical form parses"),
            InstanceId(0x3f5a_9c02_d1e8_4b76_90ab_3cde_12f4_5678),
        );
        assert!("0".repeat(31).parse::<InstanceId>().is_err());
        assert!("0".repeat(33).parse::<InstanceId>().is_err());
        assert!(
            "3F5A9C02D1E84B7690AB3CDE12F45678"
                .parse::<InstanceId>()
                .is_err()
        );
        assert!(
            "+0000000000000000000000000000001"
                .parse::<InstanceId>()
                .is_err()
        );
        assert!(
            "3f5a9c02d1e84b7690ab3cde12f4567g"
                .parse::<InstanceId>()
                .is_err()
        );
        assert!("".parse::<InstanceId>().is_err());
    }

    /// Asserts that `Display` renders the zero-padded 32-digit form and
    /// that `Debug` renders the same spelling rather than the decimal one
    /// the derive would produce.
    ///
    /// Case: an operator reads a `tracing` line naming an instance and
    /// compares it against the bytes their program wrote to the PTY.
    #[test]
    fn an_instance_id_renders_as_its_wire_spelling() {
        let id = InstanceId(1);
        assert_eq!(id.to_string(), "00000000000000000000000000000001");
        assert_eq!(
            format!("{id:?}"),
            "InstanceId(00000000000000000000000000000001)"
        );
    }

    /// Asserts that the largest wire spelling is representable, so the
    /// parser needs no overflow branch.
    ///
    /// Case: a mount names the maximal id the 32-digit grammar admits.
    #[test]
    fn the_widest_wire_spelling_is_representable() {
        let max = "f".repeat(32);
        assert_eq!(
            max.parse::<InstanceId>().expect("max parses"),
            InstanceId(u128::MAX)
        );
        assert_eq!(InstanceId(u128::MAX).to_string(), max);
    }
}
