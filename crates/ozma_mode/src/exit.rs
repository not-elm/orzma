//! Child-process exit observer: sends `AppExit` when the shell quits.

use bevy::prelude::*;
use ozma_tty_engine::TerminalChildExit;

/// Observer fired when the PTY child exits. Sends `AppExit::Success`.
pub(crate) fn on_child_exit(_ev: On<TerminalChildExit>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}
