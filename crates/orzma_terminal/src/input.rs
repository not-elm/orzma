//! Encoders that turn user input events into the byte sequences written to
//! the PTY.

use crate::input::keyboard::{TerminalKey, TerminalModifiers};

mod keyboard;

pub struct PtyInput(Vec<u8>);

impl PtyInput {
    pub fn encode_key(
        key: &TerminalKey,
        mods: &TerminalModifiers,
        app_cursor_keys: bool,
    ) -> Option<Self> {
        let raw = keyboard::encode_key(key, mods, app_cursor_keys)?;
        Some(Self(raw))
    }
}
