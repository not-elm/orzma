//! The multiplexer's thread-facing half: the command vocabulary the GUI
//! sends, and the loop that waits on the command channel and every
//! pane's PTY streams.

use crate::backend::queue_sample::QueueSampler;
use crate::backend::{
    Backend, CommandSeq, NewPaneAt, OrzmuxEvent, PaneDirection, PaneId, PaneTarget, RequestId,
    SplitId, log_refused_command,
};
use crate::error::OrzmuxResult;
use crossbeam_channel::{Receiver, Select, Sender, TryRecvError};
use orzma_tty::prelude::{MouseReport, TerminalKey, TerminalModifiers, WheelInput};
use orzma_tty::{CellPixels, EnvKey, EnvValue};
use orzma_vt::prelude::{
    CellSide, GridColumn, GridPoint, GridSize, InstanceId, PlacementSize, ScreenLine, Scroll,
    SelectionKind,
};
use std::path::PathBuf;
use std::time::Instant;

/// A command the GUI sends to the backend.
#[derive(Debug, Clone)]
pub enum OrzmuxCommand {
    /// The whole window's size in cells plus the cell pixel pitch.
    Resize {
        /// The window's size in cells.
        size: GridSize,
        /// The pixel size of one cell, used to derive the PTY winsize.
        cell_px: CellPixels,
    },
    /// Spawn a pane. `env` is forwarded to the shell verbatim.
    ///
    /// A split with `cwd: None` starts in the target pane's working
    /// directory: on Unix the directory of its foreground process or
    /// shell when the OS reports one, else the directory it last
    /// reported through OSC 7 or OSC 9;9, else the directory it was
    /// spawned in; on Windows the report is preferred over the OS. Only
    /// a directory that still exists and can be entered is used. A root
    /// pane with `cwd: None`, or a split whose target has none of these,
    /// starts in the user's home directory.
    NewPane {
        /// The id the resulting `PaneOpened` / `SpawnFailed` correlates to.
        request: RequestId,
        /// Where the new pane goes in the layout tree.
        at: NewPaneAt,
        /// The working directory to spawn the shell in, when given.
        cwd: Option<PathBuf>,
        /// Extra environment variables forwarded to the shell.
        env: Vec<(EnvKey, EnvValue)>,
    },
    /// Terminate a pane and remove it from the layout tree.
    KillPane {
        /// The pane to kill.
        pane: PaneTarget,
    },
    /// Make a pane the backend's active pane.
    SelectPane {
        /// The pane to activate.
        pane: PaneId,
    },
    /// Move the active pane to its neighbour in the given direction.
    SelectPaneDirection {
        /// The neighbour direction to select.
        direction: PaneDirection,
    },
    /// Sets whether the primary window has keyboard focus. The active pane
    /// holds focus only while the window does, and the window counts as
    /// focused until the first `WindowFocus` arrives.
    WindowFocus {
        /// The focus state to apply.
        focused: bool,
    },
    /// Forward a key press to a pane's PTY.
    KeyInput {
        /// The pane receiving the key.
        pane: PaneTarget,
        /// The key pressed.
        key: TerminalKey,
        /// The modifier keys held alongside `key`.
        mods: TerminalModifiers,
    },
    /// Forward pasted text to a pane.
    Paste {
        /// The pane receiving the paste.
        pane: PaneTarget,
        /// The pasted text.
        text: String,
    },
    /// Forward a mouse report to a pane's PTY. The pane writes nothing
    /// while its VT has no mouse tracking level in force.
    MouseInput {
        /// The pane receiving the mouse event.
        pane: PaneId,
        /// The mouse report to encode.
        report: MouseReport,
    },
    /// Route one frame's wheel notches over a pane by the pane's live VT
    /// modes.
    Wheel {
        /// The pane under the cursor.
        pane: PaneId,
        /// The notches and the modifiers and cell they were gathered with.
        input: WheelInput,
    },
    /// Scroll a pane's viewport.
    Scroll {
        /// The pane to scroll.
        pane: PaneId,
        /// The scroll motion to apply.
        scroll: Scroll,
    },
    /// Begin a selection in a pane.
    SelectionStart {
        /// The pane the selection starts in.
        pane: PaneId,
        /// The cell the selection anchors at.
        cell: GridPoint,
        /// Which half of the anchor cell the press landed on.
        side: CellSide,
        /// The selection's granularity (cell, word, line).
        kind: SelectionKind,
    },
    /// Extend an in-progress selection to a new cell.
    SelectionUpdate {
        /// The pane whose selection is extended.
        pane: PaneId,
        /// The cell the selection now extends to.
        cell: GridPoint,
        /// Which half of the target cell the drag landed on.
        side: CellSide,
    },
    /// Clear a pane's selection.
    SelectionClear {
        /// The pane whose selection is cleared.
        pane: PaneId,
    },
    /// Read back the text of a pane's current selection.
    CopySelection {
        /// The pane to read the selection from.
        pane: PaneTarget,
    },
    /// Release webview placement instances a pane no longer displays.
    RemovePlacements {
        /// The pane the placements belong to.
        pane: PaneId,
        /// The placement instances to release.
        instances: Vec<InstanceId>,
    },
    /// Move a split's divider.
    ResizeSplit {
        /// The split whose divider moves.
        split: SplitId,
        /// The whole-window cell boundary to put the divider on: `x` for
        /// a vertical split, `y` for a horizontal one.
        position: u16,
    },
    /// Register a host-driven webview placement at a visible cell of a
    /// pane — the socket-op counterpart of the APC `mount` for PTYs that
    /// drop APC (ConPTY).
    MountPlacement {
        /// The pane the placement belongs to.
        pane: PaneId,
        /// The host-minted instance the mount registers.
        instance: InstanceId,
        /// The visible row the rect's top edge sits on.
        row: ScreenLine,
        /// The column the rect's left edge sits on.
        column: GridColumn,
        /// The rect's extent in cells.
        size: PlacementSize,
    },
}

impl OrzmuxCommand {
    /// The variant's name, as the refusal log line prints it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Resize { .. } => "Resize",
            Self::NewPane { .. } => "NewPane",
            Self::KillPane { .. } => "KillPane",
            Self::SelectPane { .. } => "SelectPane",
            Self::SelectPaneDirection { .. } => "SelectPaneDirection",
            Self::WindowFocus { .. } => "WindowFocus",
            Self::KeyInput { .. } => "KeyInput",
            Self::Paste { .. } => "Paste",
            Self::MouseInput { .. } => "MouseInput",
            Self::Wheel { .. } => "Wheel",
            Self::Scroll { .. } => "Scroll",
            Self::SelectionStart { .. } => "SelectionStart",
            Self::SelectionUpdate { .. } => "SelectionUpdate",
            Self::SelectionClear { .. } => "SelectionClear",
            Self::CopySelection { .. } => "CopySelection",
            Self::RemovePlacements { .. } => "RemovePlacements",
            Self::ResizeSplit { .. } => "ResizeSplit",
            Self::MountPlacement { .. } => "MountPlacement",
        }
    }

    /// The pane the command addresses, or `None` when it addresses the
    /// window rather than one pane.
    pub fn target(&self) -> Option<PaneTarget> {
        match self {
            Self::KillPane { pane }
            | Self::KeyInput { pane, .. }
            | Self::Paste { pane, .. }
            | Self::CopySelection { pane } => Some(*pane),
            Self::SelectPane { pane } => Some(PaneTarget::Id(*pane)),
            Self::MouseInput { pane, .. }
            | Self::Wheel { pane, .. }
            | Self::Scroll { pane, .. }
            | Self::SelectionStart { pane, .. }
            | Self::SelectionUpdate { pane, .. }
            | Self::SelectionClear { pane }
            | Self::RemovePlacements { pane, .. }
            | Self::MountPlacement { pane, .. } => Some(PaneTarget::Id(*pane)),
            Self::Resize { .. }
            | Self::NewPane { .. }
            | Self::SelectPaneDirection { .. }
            | Self::WindowFocus { .. }
            | Self::ResizeSplit { .. } => None,
        }
    }
}

/// The multiplexer's thread: owns the GUI channels and drives one
/// [`Backend`].
pub(crate) struct EventLoop {
    backend: Backend,
    commands: Receiver<(CommandSeq, OrzmuxCommand)>,
    events: Sender<OrzmuxEvent>,
    /// Set when the GUI's event receiver is gone; the loop exits.
    gui_gone: bool,
    /// What each `Select` index of the last `wait_ready` referred to.
    sources: Vec<Ready>,
    /// Per-queue peaks between samples; logged once a second.
    sampler: QueueSampler,
}

impl EventLoop {
    /// A loop that drives `backend` over the given channels.
    pub fn new(
        backend: Backend,
        commands: Receiver<(CommandSeq, OrzmuxCommand)>,
        events: Sender<OrzmuxEvent>,
    ) -> Self {
        Self {
            backend,
            commands,
            events,
            gui_gone: false,
            sources: Vec::new(),
            sampler: QueueSampler::new(Instant::now()),
        }
    }

    /// Runs until the command channel disconnects (the GUI dropped its
    /// client) or the GUI stops receiving events. Dropping the backend
    /// on return kills every child.
    pub fn run(mut self) {
        loop {
            let ready = self.wait_ready();
            self.record_queue_depths();
            let connected = match ready {
                Some(Ready::Commands) => self.drain_commands(),
                Some(Ready::Pane(pane)) => {
                    self.backend.pump_pane(pane);
                    true
                }
                None => true,
            };
            if !connected {
                self.flush_events();
                return;
            }
            self.backend.service_deadlines();
            self.flush_events();
            self.report_queue_sample(Instant::now());
            if self.gui_gone {
                return;
            }
        }
    }

    /// Applies up to `COMMAND_BATCH` queued commands. Returns `false`
    /// when the command channel is disconnected.
    pub fn drain_commands(&mut self) -> bool {
        for _ in 0..COMMAND_BATCH {
            match self.commands.try_recv() {
                Ok((seq, command)) => self.handle_command(seq, command),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        true
    }

    /// Hands the backend's queued events to the GUI, recording a gone
    /// receiver instead of failing.
    pub fn flush_events(&mut self) {
        for event in self.backend.drain_events() {
            if self.events.send(event).is_err() {
                self.gui_gone = true;
            }
        }
    }

    /// Pumps one pane and hands out whatever it produced.
    #[cfg(test)]
    pub fn pump_pane(&mut self, id: PaneId) {
        self.backend.pump_pane(id);
    }

    /// Pumps every pane whose next deadline has passed.
    #[cfg(test)]
    pub fn service_deadlines(&mut self) {
        self.backend.service_deadlines();
    }

    /// The backend this loop drives.
    #[cfg(test)]
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// Applies one command. An unresolvable target and a refused PTY
    /// write are logged and dropped; `CopySelection` always answers,
    /// `SelectPane` always publishes a layout, and `SelectPaneDirection`
    /// publishes one only when the active pane moved.
    fn handle_command(&mut self, seq: CommandSeq, command: OrzmuxCommand) {
        self.backend.set_processed(seq);
        if let OrzmuxCommand::NewPane {
            request,
            at,
            cwd,
            env,
        } = command
        {
            if let Err(error) = self.backend.open_pane(request, at, cwd, env) {
                self.backend.fail_spawn(request, &error);
            }
            return;
        }
        let name = command.name();
        let target = command.target();
        if let Err(error) = self.dispatch(command) {
            log_refused_command(name, target, &error);
        }
    }

    /// Routes one command to the backend operation that applies it.
    fn dispatch(&mut self, command: OrzmuxCommand) -> OrzmuxResult {
        match command {
            OrzmuxCommand::NewPane { .. } => Ok(()),
            OrzmuxCommand::Resize { size, cell_px } => {
                self.backend.resize(size, cell_px);
                Ok(())
            }
            OrzmuxCommand::KillPane { pane } => self.backend.kill_pane(pane),
            OrzmuxCommand::SelectPane { pane } => self.backend.select_pane(pane),
            OrzmuxCommand::SelectPaneDirection { direction } => {
                self.backend.select_pane_direction(direction);
                Ok(())
            }
            OrzmuxCommand::WindowFocus { focused } => {
                self.backend.window_focus(focused);
                Ok(())
            }
            OrzmuxCommand::ResizeSplit { split, position } => {
                self.backend.resize_split(split, position);
                Ok(())
            }
            OrzmuxCommand::KeyInput { pane, key, mods } => self.backend.key_input(pane, key, mods),
            OrzmuxCommand::Paste { pane, text } => self.backend.paste(pane, text),
            OrzmuxCommand::MouseInput { pane, report } => self.backend.mouse_input(pane, report),
            OrzmuxCommand::Wheel { pane, input } => self.backend.wheel(pane, input),
            OrzmuxCommand::Scroll { pane, scroll } => self.backend.scroll(pane, scroll),
            OrzmuxCommand::SelectionStart {
                pane,
                cell,
                side,
                kind,
            } => self.backend.selection_start(pane, cell, side, kind),
            OrzmuxCommand::SelectionUpdate { pane, cell, side } => {
                self.backend.selection_update(pane, cell, side)
            }
            OrzmuxCommand::SelectionClear { pane } => self.backend.selection_clear(pane),
            OrzmuxCommand::CopySelection { pane } => {
                self.backend.copy_selection(pane);
                Ok(())
            }
            OrzmuxCommand::RemovePlacements { pane, instances } => {
                self.backend.remove_placements(pane, instances)
            }
            OrzmuxCommand::MountPlacement {
                pane,
                instance,
                row,
                column,
                size,
            } => self
                .backend
                .mount_placement(pane, instance, row, column, size),
        }
    }

    /// Blocks until a command or a pane stream is ready, or the earliest
    /// of the panes' next deadlines and the sampler's report deadline
    /// passes. Returns the ready source, `None` on timeout.
    fn wait_ready(&mut self) -> Option<Ready> {
        let mut select = Select::new();
        self.sources.clear();
        select.recv(&self.commands);
        self.sources.push(Ready::Commands);
        for (id, readiness) in self.backend.readiness() {
            select.recv(readiness.chunks);
            self.sources.push(Ready::Pane(id));
            if let Some(exit) = readiness.exit {
                select.recv(exit);
                self.sources.push(Ready::Pane(id));
            }
        }
        let index = match self.next_wake_deadline() {
            Some(deadline) => select.ready_deadline(deadline).ok()?,
            None => select.ready(),
        };
        Some(self.sources[index])
    }

    /// The earliest of the panes' next deadlines and the sampler's report
    /// deadline, or `None` when every pane is idle and no peak waits to
    /// be reported.
    fn next_wake_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
        self.backend
            .next_deadline(now)
            .into_iter()
            .chain(self.sampler.report_deadline())
            .min()
    }

    /// Records every queue's current depth into the sampler: each pane's
    /// unread chunk count, the event channel, and the command channel.
    fn record_queue_depths(&mut self) {
        for (id, depth) in self.backend.chunk_depths() {
            self.sampler.record_pane_depth(id, depth);
        }
        self.sampler
            .record_channel_depths(self.events.len(), self.commands.len());
    }

    /// Logs the sample the sampler hands out at `now`, if one is due:
    /// one line per pane with a recorded chunk peak and one line for
    /// the channels when either has a recorded peak.
    fn report_queue_sample(&mut self, now: Instant) {
        let Some(sample) = self.sampler.sample(now) else {
            return;
        };
        for (pane, depth) in sample.chunks {
            tracing::debug!(
                target: "orzmux::queues",
                ?pane,
                depth = depth.0,
                "chunk queue peak"
            );
        }
        if sample.events > 0 || sample.commands > 0 {
            tracing::debug!(
                target: "orzmux::queues",
                events = sample.events,
                commands = sample.commands,
                "event and command queue peaks"
            );
        }
    }
}

/// What one ready `Select` index refers to.
#[derive(Debug, Clone, Copy)]
enum Ready {
    Commands,
    Pane(PaneId),
}

/// How many queued commands one iteration applies before pumping panes.
const COMMAND_BATCH: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::queue_sample::ChunkDepth;
    use crate::test_support::{FactoryLog, FakeFactory, Harness};
    use crossbeam_channel::unbounded;
    use orzma_tty::prelude::WheelConfig;
    use std::sync::Arc;

    /// Asserts that the depths recorded after a wake are the chunks
    /// still queued before the pump drains them.
    ///
    /// Case: a pane's reader queued two chunks while the backend slept
    /// and the `Select` just woke for that pane.
    #[test]
    fn record_queue_depths_sees_the_chunks_queued_before_the_pump() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.chunk_tx.send(b"a".to_vec()).unwrap();
        pane.chunk_tx.send(b"b".to_vec()).unwrap();
        h.event_loop.record_queue_depths();
        h.pump_pane(root);
        let sample = h
            .event_loop
            .sampler
            .sample(Instant::now() + QueueSampler::SAMPLE_INTERVAL)
            .expect("a peak was recorded");
        assert_eq!(sample.chunks, vec![(root, ChunkDepth(2))]);
    }

    /// Asserts that the wake deadline is the sampler's report deadline
    /// while every pane is idle, and the earlier coalescer deadline
    /// once a pane has output pending.
    ///
    /// Case: a burst of output was recorded, then every pane went idle
    /// before the second elapsed, and the peak still has to be logged.
    #[test]
    fn the_wake_deadline_is_the_report_deadline_when_no_pane_deadline_is_earlier() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        h.pump_pane(root);
        h.drain();
        assert_eq!(
            h.event_loop.next_wake_deadline(),
            None,
            "precondition: the pane is idle after its bootstrap frame and no peak is recorded"
        );
        h.event_loop.sampler.record_pane_depth(root, ChunkDepth(2));
        let report_deadline = h.event_loop.sampler.report_deadline();
        assert!(report_deadline.is_some());
        assert_eq!(h.event_loop.next_wake_deadline(), report_deadline);
        pane.chunk_tx.send(b"x".to_vec()).unwrap();
        h.pump_pane(root);
        let pane_deadline = h
            .backend()
            .pane(root)
            .expect("the root pane")
            .tty
            .next_deadline(Instant::now())
            .expect("pending output arms the coalescer");
        assert!(Some(pane_deadline) < report_deadline);
        assert_eq!(h.event_loop.next_wake_deadline(), Some(pane_deadline));
    }

    /// Asserts that the events of the final command batch still reach the
    /// GUI when the command channel disconnects in the same iteration.
    ///
    /// Case: the GUI drops its client right after asking for a pane's
    /// selection, and reads the answer off the event channel afterwards.
    #[test]
    fn the_final_batch_reaches_the_gui_after_a_disconnect() {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let (spawned_tx, _spawned_rx) = unbounded();
        let factory = FakeFactory {
            spawned: spawned_tx,
            log: Arc::new(FactoryLog::default()),
        };
        let backend = Backend::new(Box::new(factory), WheelConfig::default());
        command_tx
            .send((
                CommandSeq(1),
                OrzmuxCommand::CopySelection {
                    pane: PaneTarget::Active,
                },
            ))
            .expect("the loop still holds the receiver");
        drop(command_tx);
        EventLoop::new(backend, command_rx, event_tx).run();
        let events: Vec<OrzmuxEvent> = event_rx.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OrzmuxEvent::SelectionText { text: None }))
        );
    }
}
