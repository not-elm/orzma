//! Parsed control-mode events and the typed entity ids they carry.

/// A tmux pane id (`%N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

/// A tmux window id (`@N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u32);

/// A tmux session id (`$N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u32);

/// A single parsed control-mode line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// `%begin <time> <number> <flags>` — start of a command output block.
    Begin { time: u64, number: u32, flags: u32 },
    /// `%end <time> <number> <flags>` — successful end of a command output block.
    End { time: u64, number: u32, flags: u32 },
    /// `%error <time> <number> <flags>` — failed end of a command output block.
    Error { time: u64, number: u32, flags: u32 },

    /// `%output <pane> <data>` — pane output, octal-decoded to raw bytes.
    Output { pane: PaneId, data: Vec<u8> },
    /// `%extended-output <pane> <age> ... : <data>` — output with buffering age (ms).
    ExtendedOutput { pane: PaneId, age: u64, data: Vec<u8> },

    /// `%window-add <window>`.
    WindowAdd { window: WindowId },
    /// `%window-close <window>`.
    WindowClose { window: WindowId },
    /// `%window-renamed <window> <name>`.
    WindowRenamed { window: WindowId, name: String },
    /// `%window-pane-changed <window> <pane>`.
    WindowPaneChanged { window: WindowId, pane: PaneId },
    /// `%unlinked-window-add <window>`.
    UnlinkedWindowAdd { window: WindowId },
    /// `%unlinked-window-close <window>`.
    UnlinkedWindowClose { window: WindowId },
    /// `%unlinked-window-renamed <window>` (no name arg in tmux 3.6a).
    UnlinkedWindowRenamed { window: WindowId },
    /// `%pane-mode-changed <pane>`.
    PaneModeChanged { pane: PaneId },

    /// `%session-changed <session> <name>`.
    SessionChanged { session: SessionId, name: String },
    /// `%session-renamed <name>`.
    SessionRenamed { name: String },
    /// `%session-window-changed <session> <window>`.
    SessionWindowChanged { session: SessionId, window: WindowId },
    /// `%sessions-changed`.
    SessionsChanged,

    /// `%client-detached <client>`.
    ClientDetached { client: String },
    /// `%client-session-changed <client> <session> <name>`.
    ClientSessionChanged { client: String, session: SessionId, name: String },
    /// `%layout-change <window> <layout> <visible-layout> <flags>`.
    LayoutChange { window: WindowId, layout: String, visible_layout: String, flags: String },
    /// `%continue <pane>`.
    Continue { pane: PaneId },
    /// `%pause <pane>`.
    Pause { pane: PaneId },
    /// `%exit [reason]`.
    Exit { reason: Option<String> },
    /// `%message <message>`.
    Message { message: String },
    /// `%config-error <message>`.
    ConfigError { message: String },
    /// `%paste-buffer-changed <name>`.
    PasteBufferChanged { name: String },
    /// `%paste-buffer-deleted <name>`.
    PasteBufferDeleted { name: String },
    /// `%subscription-changed <name> <session> <window> <index> <pane> ... : <value>`.
    SubscriptionChanged {
        name: String,
        session: SessionId,
        window: WindowId,
        window_index: i32,
        pane: PaneId,
        value: String,
    },

    /// An unrecognised `%keyword rest` line, kept for forward compatibility.
    Unknown { name: String, rest: String },
}
