/// 1-indexed cell coordinate suitable for SGR / X10 mouse reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellCoord {
    pub col: u32,
    pub row: u32,
}

/// Shared mouse-protocol modifier set. `WheelModifiers` builds one of
/// these at the encoder call boundary; `ButtonAction::route` already
/// uses this type natively.
///
/// `WheelModifiers::fine` is NOT part of the protocol — it is router
/// policy that decides line counts — so it stays in `WheelModifiers`
/// and does not appear here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProtocolModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Encodes one mouse-protocol report.
///
/// `cb_base` is the raw button code: 0/1/2 = L/M/R press, 64/65 = wheel
/// up/down. Caller is responsible for setting `motion = true` for drag
/// and for wheel reports (xterm treats wheel as a motion-bit press).
pub(super) fn encode_protocol_event(
    modes: TermMode,
    cb_base: u8,
    cell: CellCoord,
    mods: ProtocolModifiers,
    motion: bool,
    release: bool,
) -> Vec<u8> {
    if modes.contains(TermMode::SGR_MOUSE) {
        encode_sgr(cb_base, cell, mods, motion, release)
    } else {
        encode_x10(cb_base, cell, mods, motion, release)
    }
}

fn encode_sgr(
    cb_base: u8,
    cell: CellCoord,
    mods: ProtocolModifiers,
    motion: bool,
    release: bool,
) -> Vec<u8> {
    let mut cb: u32 = cb_base as u32;
    if motion {
        cb += 32;
    }
    if mods.shift {
        cb += 4;
    }
    if mods.alt || mods.meta {
        cb += 8;
    }
    if mods.ctrl {
        cb += 16;
    }
    let final_byte = if release { 'm' } else { 'M' };
    format!(
        "\x1b[<{};{};{}{}",
        cb,
        cell.col.max(1),
        cell.row.max(1),
        final_byte
    )
    .into_bytes()
}

fn encode_x10(
    cb_base: u8,
    cell: CellCoord,
    mods: ProtocolModifiers,
    motion: bool,
    release: bool,
) -> Vec<u8> {
    let mut cb: u32 = if release { 3 } else { cb_base as u32 };
    if motion {
        cb += 32;
    }
    if mods.shift {
        cb += 4;
    }
    if mods.alt || mods.meta {
        cb += 8;
    }
    if mods.ctrl {
        cb += 16;
    }
    let col = cell.col.clamp(1, 223) as u8;
    let row = cell.row.clamp(1, 223) as u8;
    vec![0x1b, b'[', b'M', (cb + 32) as u8, col + 32, row + 32]
}
