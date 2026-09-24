//! The multiplexer's thread-facing half: the command vocabulary the GUI
//! sends, and the loop that waits on the command channel and every
//! pane's PTY streams.

use crate::backend::queue_sample::QueueSampler;
use crate::backend::{
    Backend, CommandSeq, NewPaneAt, PaneDirection, PaneId, PaneTarget, RequestId, SplitId,
    log_refused_write,
};
use crate::error::{OrzmuxError, OrzmuxResult};
use crossbeam_channel::{Receiver, Select, TryRecvError};
use orzma_tty::prelude::{PointerInput, TerminalKey, TerminalModifiers, WheelInput};
use orzma_tty::{CellPixels, EnvKey, EnvValue};
use orzma_vt::prelude::{GridColumn, GridSize, InstanceId, PlacementSize, ScreenLine, Scroll};
use std::path::PathBuf;
use std::time::Instant;
use tracing::Level;

mod gui_link;

pub(crate) use gui_link::GuiLink;

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
    /// Route one frame's wheel notches over a pane by the pane's live VT
    /// modes.
    Wheel {
        /// The pane under the cursor.
        pane: PaneId,
        /// The notches and the modifiers and cell they were gathered with.
        input: WheelInput,
    },
    /// Routes one pointer event over a pane by the pane's live VT modes:
    /// a mouse report for the application, or the pane's own selection.
    Pointer {
        /// The pane the pointer event addresses.
        pane: PaneId,
        /// The pointer event.
        input: PointerInput,
    },
    /// Scroll a pane's viewport.
    Scroll {
        /// The pane to scroll.
        pane: PaneId,
        /// The scroll motion to apply.
        scroll: Scroll,
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
    /// The variant's name and the pane it addresses, as the refusal log
    /// line prints them. The pane is `None` for a command that addresses
    /// the window rather than one pane.
    pub(crate) fn log_context(&self) -> (&'static str, Option<PaneTarget>) {
        match self {
            Self::Resize { .. } => ("Resize", None),
            Self::NewPane { .. } => ("NewPane", None),
            Self::KillPane { pane } => ("KillPane", Some(*pane)),
            Self::SelectPane { pane } => ("SelectPane", Some(PaneTarget::Id(*pane))),
            Self::SelectPaneDirection { .. } => ("SelectPaneDirection", None),
            Self::WindowFocus { .. } => ("WindowFocus", None),
            Self::KeyInput { pane, .. } => ("KeyInput", Some(*pane)),
            Self::Paste { pane, .. } => ("Paste", Some(*pane)),
            Self::Wheel { pane, .. } => ("Wheel", Some(PaneTarget::Id(*pane))),
            Self::Pointer { pane, .. } => ("Pointer", Some(PaneTarget::Id(*pane))),
            Self::Scroll { pane, .. } => ("Scroll", Some(PaneTarget::Id(*pane))),
            Self::SelectionClear { pane } => ("SelectionClear", Some(PaneTarget::Id(*pane))),
            Self::CopySelection { pane } => ("CopySelection", Some(*pane)),
            Self::RemovePlacements { pane, .. } => {
                ("RemovePlacements", Some(PaneTarget::Id(*pane)))
            }
            Self::ResizeSplit { .. } => ("ResizeSplit", None),
            Self::MountPlacement { pane, .. } => ("MountPlacement", Some(PaneTarget::Id(*pane))),
        }
    }

    /// Whether this command, arriving right after `earlier`, leaves
    /// nothing for `earlier` to do: a window resize replaces the resize
    /// before it, and a divider move the move of the same divider before
    /// it.
    fn supersedes(&self, earlier: &Self) -> bool {
        match (earlier, self) {
            (Self::Resize { .. }, Self::Resize { .. }) => true,
            (Self::ResizeSplit { split: moved, .. }, Self::ResizeSplit { split, .. }) => {
                moved == split
            }
            _ => false,
        }
    }
}

/// The multiplexer's thread: owns the GUI channels and drives one
/// [`Backend`].
pub(crate) struct EventLoop {
    backend: Backend,
    commands: Receiver<(CommandSeq, OrzmuxCommand)>,
    /// The GUI's event channel, which wakes the GUI after each flush that
    /// sends an event.
    gui: GuiLink,
    /// Set when the GUI's event receiver is gone; the loop exits.
    gui_gone: bool,
    /// What each `Select` index of the last `wait_ready` referred to.
    sources: Vec<Ready>,
    /// Per-queue peaks between samples; logged once a second.
    sampler: QueueSampler,
}

impl EventLoop {
    /// A loop that drives `backend`, reading commands from `commands` and
    /// sending events through `gui`.
    pub fn new(
        backend: Backend,
        commands: Receiver<(CommandSeq, OrzmuxCommand)>,
        gui: GuiLink,
    ) -> Self {
        Self {
            backend,
            commands,
            gui,
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
            if connected {
                self.backend.service_deadlines();
            }
            self.flush_events();
            self.report_queue_sample(Instant::now());
            if !connected || self.gui_gone {
                return;
            }
        }
    }

    /// Applies up to `COMMAND_BATCH` queued commands, applying only the
    /// last of a run of consecutive `Resize` commands, and only the last
    /// of a run of consecutive `ResizeSplit` commands for the same split.
    /// Returns `false` when the command channel is disconnected.
    pub fn drain_commands(&mut self) -> bool {
        let mut held: Option<(CommandSeq, OrzmuxCommand)> = None;
        let mut connected = true;
        for _ in 0..COMMAND_BATCH {
            match self.commands.try_recv() {
                Ok(next) => {
                    if let Some((seq, command)) = held
                        .take()
                        .filter(|(_, earlier)| !next.1.supersedes(earlier))
                    {
                        self.handle_command(seq, command);
                    }
                    held = Some(next);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    connected = false;
                    break;
                }
            }
        }
        if let Some((seq, command)) = held {
            self.handle_command(seq, command);
        }
        connected
    }

    /// Hands the backend's queued events to the GUI, waking it only when it
    /// sent any, and records a gone receiver instead of failing.
    pub fn flush_events(&mut self) {
        if !self.gui.send_batch(self.backend.drain_events()) {
            self.gui_gone = true;
        }
    }

    /// The backend this loop drives.
    #[cfg(test)]
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// The backend this loop drives, for a test that drives it directly.
    #[cfg(test)]
    pub fn backend_mut(&mut self) -> &mut Backend {
        &mut self.backend
    }

    /// Applies one command. An unresolvable target and a refused PTY
    /// write are logged and dropped; `CopySelection` always answers,
    /// `SelectPane` always publishes a layout, and `SelectPaneDirection`
    /// publishes one only when the active pane moved.
    fn handle_command(&mut self, seq: CommandSeq, command: OrzmuxCommand) {
        self.backend.set_processed(seq);
        let (name, target) = command.log_context();
        if let Err(error) = self.dispatch(command) {
            log_refused_command(name, target, &error);
        }
    }

    /// Routes one command to the backend operation that applies it. A
    /// refused `NewPane` is answered with `SpawnFailed` rather than
    /// reported to the caller.
    fn dispatch(&mut self, command: OrzmuxCommand) -> OrzmuxResult {
        match command {
            OrzmuxCommand::NewPane {
                request,
                at,
                cwd,
                env,
            } => {
                if let Err(error) = self.backend.open_pane(request, at, cwd, env) {
                    self.backend.fail_spawn(request, &error);
                }
                Ok(())
            }
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
            OrzmuxCommand::Wheel { pane, input } => self.backend.wheel(pane, input),
            OrzmuxCommand::Pointer { pane, input } => self.backend.pointer(pane, input),
            OrzmuxCommand::Scroll { pane, scroll } => self.backend.scroll(pane, scroll),
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
            .record_channel_depths(self.gui.depth(), self.commands.len());
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

/// Logs a command the backend refused, at the level its failure earns.
///
/// An unresolvable target logs at `DEBUG`, a refused PTY write goes
/// through [`log_refused_write`] at `ERROR`, and every other refusal
/// logs at `WARN`.
fn log_refused_command(name: &'static str, target: Option<PaneTarget>, error: &OrzmuxError) {
    match error {
        OrzmuxError::UnresolvedTarget => match target {
            Some(target) => {
                tracing::debug!(
                    ?target,
                    command = name,
                    "pane command dropped: no such pane"
                );
            }
            None => {
                tracing::debug!(command = name, "pane command dropped: no such pane");
            }
        },
        OrzmuxError::PtyWrite { pane, source } => {
            log_refused_write(*pane, name, source, Level::ERROR);
        }
        _ => tracing::warn!(command = name, %error, "command refused"),
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
    use crate::backend::{CloseReason, Layout, OrzmuxEvent, SplitOrientation};
    use crate::test_support::{FactoryLog, FakeFactory, FakePane, Harness};
    use crossbeam_channel::{RecvTimeoutError, bounded, unbounded};
    use orzma_tty::prelude::{
        CellCoord, KeyText, OrzmaTty, PointerButton, PointerInput, PointerKind, ProtocolModifiers,
        WheelConfig, WheelModifiers,
    };
    use orzma_tty::test_support::BlockingSink;
    use orzma_vt::Vt;
    use orzma_vt::prelude::{CellSide, OrzmaVt};
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Arc;
    use std::task::Waker;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Asserts that the depths recorded after a wake are the chunks
    /// still queued before the pump drains them.
    ///
    /// Case: a pane's reader queued two chunks while the backend slept
    /// and the `Select` just woke for that pane.
    #[test]
    fn record_queue_depths_sees_the_chunks_queued_before_the_pump() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.print(b"a");
        pane.print(b"b");
        h.event_loop_mut().record_queue_depths();
        h.pump_pane(root);
        let sample = h
            .event_loop_mut()
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
            h.event_loop().next_wake_deadline(),
            None,
            "precondition: the pane is idle after its bootstrap frame and no peak is recorded"
        );
        h.event_loop_mut()
            .sampler
            .record_pane_depth(root, ChunkDepth(2));
        let report_deadline = h.event_loop().sampler.report_deadline();
        assert!(report_deadline.is_some());
        assert_eq!(h.event_loop().next_wake_deadline(), report_deadline);
        pane.print(b"x");
        h.pump_pane(root);
        let pane_deadline = h
            .backend()
            .pane(root)
            .expect("the root pane")
            .tty
            .next_deadline(Instant::now())
            .expect("pending output arms the coalescer");
        assert!(Some(pane_deadline) < report_deadline);
        assert_eq!(h.event_loop().next_wake_deadline(), Some(pane_deadline));
    }

    /// Asserts that the events of the final command batch still reach the
    /// GUI when the command channel disconnects in the same iteration.
    ///
    /// Case: the GUI drops its client right after asking for a pane's
    /// selection, and reads the answer off the event channel afterwards.
    #[test]
    fn the_final_batch_reaches_the_gui_after_a_disconnect() {
        let (command_tx, command_rx) = unbounded();
        let (gui, event_rx) = GuiLink::channel(Waker::noop().clone());
        let (spawned_tx, _spawned_rx) = unbounded();
        let factory = FakeFactory::new(spawned_tx, Arc::new(FactoryLog::default()));
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
        EventLoop::new(backend, command_rx, gui).run();
        let events: Vec<OrzmuxEvent> = event_rx.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OrzmuxEvent::SelectionText { text: None }))
        );
    }

    /// Asserts that `NewPane { Root }` before any `Resize` fails instead
    /// of guessing a size.
    ///
    /// Case: a misordered GUI start-up spawns before the window metrics
    /// exist.
    #[test]
    fn a_root_pane_before_geometry_is_refused() {
        let mut h = Harness::new();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(9),
            at: NewPaneAt::Root,
            cwd: None,
            env: vec![],
        });
        let events = h.drain();
        assert!(matches!(
            events.front(),
            Some(OrzmuxEvent::SpawnFailed {
                request: RequestId(9),
                ..
            })
        ));
    }

    /// Asserts the `PaneOpened → Layout` order for the first pane, with
    /// the pane spawned at the solved window size.
    ///
    /// Case: the app starts and opens its first shell in an 80×24 window.
    #[test]
    fn the_root_pane_opens_at_the_window_size_and_reports_a_layout() {
        let mut h = Harness::new();
        h.resize(GridSize::new(80, 24).expect("a valid size"));
        h.drain();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(1),
            at: NewPaneAt::Root,
            cwd: None,
            env: vec![],
        });
        let mut events = h.drain();
        assert!(matches!(
            events.pop_front(),
            Some(OrzmuxEvent::PaneOpened {
                request: RequestId(1),
                ..
            })
        ));
        let Some(OrzmuxEvent::Layout { layout, frames }) = events.pop_front() else {
            panic!("expected Layout");
        };
        assert_eq!(layout.panes.len(), 1);
        assert_eq!((layout.panes[0].cols, layout.panes[0].rows), (80, 24));
        assert_eq!(layout.active, Some(layout.panes[0].pane));
        assert!(frames.is_empty(), "a fresh pane has no resized sibling");
    }

    /// Asserts that a failed spawn rolls the tree back and reports
    /// `SpawnFailed` without a `Layout`.
    ///
    /// Case: the shell binary is missing when the user splits a pane.
    #[test]
    fn a_failed_split_spawn_rolls_back_and_reports_spawn_failed() {
        let mut h = Harness::new();
        let (root, _pane) = h.open_root();
        h.fail_next_spawn();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Id(root),
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let events = h.drain();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events.front(),
            Some(OrzmuxEvent::SpawnFailed {
                request: RequestId(2),
                ..
            })
        ));
        assert_eq!(h.backend().tree().panes(), vec![root]);
        assert_eq!(h.backend().tree().active(), Some(root));
    }

    /// Asserts that a split whose target no longer exists is answered
    /// with `SpawnFailed` alone, leaving the tree and the next pane id
    /// untouched.
    ///
    /// Case: the target pane's shell exits between the moment the user
    /// presses the split shortcut and the moment the backend reaches the
    /// command.
    #[test]
    fn a_split_whose_target_is_gone_is_refused_without_disturbing_the_tree() {
        let mut h = Harness::new();
        let (root, _pane) = h.open_root();
        let next_id = h.backend().next_pane_id();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Id(PaneId(9999)),
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let events = h.drain();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events.front(),
            Some(OrzmuxEvent::SpawnFailed {
                request: RequestId(2),
                ..
            })
        ));
        assert_eq!(h.backend().tree().panes(), vec![root]);
        assert_eq!(h.backend().next_pane_id(), next_id);
    }

    /// Asserts that a split resizes the target pane and ships its
    /// repaint inside the `Layout` event.
    ///
    /// Case: the user splits the only pane vertically.
    #[test]
    fn a_split_resizes_the_target_and_bundles_its_frame_with_the_layout() {
        let mut h = Harness::new();
        let (root, _pane) = h.open_root();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let mut events = h.drain();
        assert!(matches!(
            events.pop_front(),
            Some(OrzmuxEvent::PaneOpened {
                request: RequestId(2),
                ..
            })
        ));
        let Some(OrzmuxEvent::Layout { layout, frames }) = events.pop_front() else {
            panic!("expected Layout");
        };
        assert_eq!(layout.panes.len(), 2);
        assert_eq!(frames.len(), 1, "only the shrunk root pane repaints");
        assert_eq!(frames[0].0, root);
        assert_eq!(frames[0].1.size, GridSize { cols: 40, rows: 24 });
        assert_eq!(
            h.backend()
                .pane(root)
                .expect("the root pane")
                .tty
                .pty_size()
                .cols,
            40
        );
        assert_eq!(h.last_spawn_size(), Some(GridSize { cols: 39, rows: 24 }));
    }

    /// Asserts that a window resize re-solves every pane and bundles the
    /// changed panes' frames with the layout.
    ///
    /// Case: the user drags the window wider with two panes open.
    #[test]
    fn a_window_resize_reflows_every_pane() {
        let mut h = Harness::new();
        let (root, _pane) = h.open_root();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        h.drain();
        h.resize(GridSize::new(120, 24).expect("a valid size"));
        let mut events = h.drain();
        let Some(OrzmuxEvent::Layout { layout, frames }) = events.pop_front() else {
            panic!("expected Layout");
        };
        assert_eq!(
            layout.size,
            GridSize {
                cols: 120,
                rows: 24
            }
        );
        assert_eq!(frames.len(), 2);
        assert_eq!(
            h.backend()
                .pane(root)
                .expect("the root pane")
                .tty
                .pty_size()
                .cols,
            60
        );
        assert_eq!(
            h.backend()
                .pane(root)
                .expect("the root pane")
                .tty
                .pty_size()
                .pixel_width,
            60 * 8
        );
    }

    /// Asserts that back-to-back resizes apply only the last one.
    ///
    /// Case: the user drags the window edge, so the GUI queues several
    /// sizes before the loop wakes.
    #[test]
    fn back_to_back_resizes_apply_only_the_last() {
        let mut h = Harness::new();
        let (root, _pane) = h.open_root();
        h.drain();
        for cols in [70, 60, 50] {
            h.queue_resize(GridSize::new(cols, 24).expect("a valid size"));
        }
        h.event_loop_mut().drain_commands();
        let layouts = h
            .drain()
            .into_iter()
            .filter(|event| matches!(event, OrzmuxEvent::Layout { .. }))
            .count();
        assert_eq!(layouts, 1);
        assert_eq!(
            h.backend()
                .pane(root)
                .expect("the root pane")
                .tty
                .pty_size()
                .cols,
            50
        );
    }

    /// Asserts that back-to-back moves of one divider apply only the last.
    ///
    /// Case: the user drags a divider between two panes, so the GUI queues
    /// several positions before the loop wakes.
    #[test]
    fn back_to_back_divider_moves_apply_only_the_last() {
        let mut h = Harness::new();
        let (_root, _pane) = h.open_root();
        split_active(&mut h, 2);
        let window = GridSize::new(80, 24).expect("a valid size");
        let split = h.backend().tree().solve(window).separators[0].split;
        for position in [50, 55, 60] {
            h.queue(OrzmuxCommand::ResizeSplit { split, position });
        }
        h.event_loop_mut().drain_commands();
        let layouts: Vec<Layout> = h
            .drain()
            .into_iter()
            .filter_map(|event| match event {
                OrzmuxEvent::Layout { layout, .. } => Some(layout),
                _ => None,
            })
            .collect();
        assert_eq!(layouts.len(), 1);
        assert_eq!(layouts[0].separators[0].x, 60);
    }

    /// Asserts that a pane whose coalescer deadline passed is pumped by
    /// `service_deadlines` even though no chunk is ready.
    ///
    /// Case: another pane floods output so the select never times out,
    /// while this pane's 12 ms cap already elapsed.
    #[test]
    fn service_deadlines_pumps_a_pane_whose_deadline_passed() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        h.pump_pane(root);
        h.drain();
        pane.print(b"hello");
        h.pump_pane(root);
        h.drain();
        thread::sleep(Duration::from_millis(15));
        h.service_deadlines();
        let events = h.drain();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OrzmuxEvent::Frame { pane, .. } if *pane == root))
        );
    }

    /// The kind of each event, for asserting an order.
    fn event_kinds(events: &VecDeque<OrzmuxEvent>) -> Vec<&'static str> {
        events
            .iter()
            .map(|event| match event {
                OrzmuxEvent::Signal { .. } => "signal",
                OrzmuxEvent::Frame { .. } => "frame",
                _ => "other",
            })
            .collect()
    }

    /// Asserts that the frame a closed synchronized update yields
    /// reaches the GUI between the signals raised before and after the
    /// close.
    ///
    /// Case: a program rings the bell inside a synchronized update,
    /// closes it, and sets the window title right behind it in the same
    /// write.
    #[test]
    fn a_closed_update_frame_arrives_between_its_signals() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        h.pump_pane(root);
        h.drain();
        thread::sleep(OrzmaTty::<OrzmaVt>::SYNC_EMIT_INTERVAL);
        pane.print(b"\x1b[?2026h\x07a\x1b[?2026l\x1b]2;t\x07");
        h.pump_pane(root);
        assert_eq!(event_kinds(&h.drain()), ["signal", "frame", "signal"]);
    }

    /// Asserts that a pane with an open synchronized update is not
    /// painted when its coalescer deadline passes.
    ///
    /// Case: nvim is halfway through a redraw inside a synchronized
    /// update when the 12 ms cap elapses.
    #[test]
    fn an_open_update_holds_the_pane_frame_back() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        h.pump_pane(root);
        h.drain();
        pane.print(b"\x1b[?2026hhello");
        h.pump_pane(root);
        thread::sleep(Duration::from_millis(15));
        h.service_deadlines();
        h.pump_pane(root);
        assert!(
            !h.drain()
                .iter()
                .any(|e| matches!(e, OrzmuxEvent::Frame { .. }))
        );
    }

    /// Asserts that a pane's last frame reaches the GUI ahead of its
    /// `PaneClosed`.
    ///
    /// Case: the shell prints a farewell line and exits before the
    /// coalesce window for that line elapsed.
    #[test]
    fn the_last_frame_precedes_the_pane_close() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        h.pump_pane(root);
        h.drain();
        pane.print(b"bye");
        pane.exit(Some(0));
        h.pump_pane(root);
        let events = h.drain();
        let frame = events
            .iter()
            .position(|e| matches!(e, OrzmuxEvent::Frame { .. }));
        let closed = events
            .iter()
            .position(|e| matches!(e, OrzmuxEvent::PaneClosed { .. }));
        assert!(frame.is_some() && frame < closed, "{events:?}");
    }

    /// An `OSC 7` sequence reporting `path`, spelled the way a shell on
    /// this platform would.
    // NOTE: the separator between the authority and the path is what
    // makes the URI well-formed. A Windows path starts with a drive
    // letter rather than `/`, so joining it to `file://localhost`
    // directly yields `file://localhostC:/…`, whose path the parser
    // reads as `/Users/…` — not drive-rooted, and rejected.
    fn osc7(path: &Path) -> Vec<u8> {
        let forward = path.display().to_string().replace('\\', "/");
        format!(
            "\x1b]7;file://localhost/{}\x1b\\",
            forward.trim_start_matches('/')
        )
        .into_bytes()
    }

    /// Splits the active pane and returns the new pane's id and its
    /// spawned fake terminal.
    fn split_active(h: &mut Harness, request: u64) -> (PaneId, FakePane) {
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(request),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let events = h.drain();
        let Some(OrzmuxEvent::PaneOpened { pane, .. }) = events.front() else {
            panic!("expected PaneOpened, got {events:?}");
        };
        (*pane, h.spawned_pane().expect("one spawned pane"))
    }

    /// Feeds `CSI ? 1004 h` through `pane`'s output stream and pumps it, so
    /// the pane's application has focus reporting enabled.
    fn enable_focus_reporting(h: &mut Harness, id: PaneId, pane: &FakePane) {
        pane.print(b"\x1b[?1004h");
        h.pump_pane(id);
        h.drain();
        assert!(
            h.backend()
                .pane(id)
                .expect("the pane")
                .tty
                .vt()
                .modes()
                .focus_in_out,
            "precondition: focus reporting is enabled"
        );
    }

    /// One wheel-up notch over cell (1, 1) with nothing held.
    fn wheel_up() -> WheelInput {
        WheelInput {
            up: 1,
            right: 0,
            mods: WheelModifiers::default(),
            cell: Some(CellCoord { col: 1, row: 1 }),
            report_mods: ProtocolModifiers::default(),
        }
    }

    /// Asserts that a `Wheel` command reaches its pane's terminal, which
    /// routes it by that pane's own modes.
    ///
    /// Case: nvim has turned on button-event tracking and SGR reports in
    /// a pane, and the user spins the wheel up over it.
    #[test]
    fn a_wheel_command_reaches_its_panes_terminal() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.print(b"\x1b[?1002h\x1b[?1006h");
        h.pump_pane(root);
        h.drain();
        h.send(OrzmuxCommand::Wheel {
            pane: root,
            input: wheel_up(),
        });
        h.settle_writes();
        assert_eq!(pane.received(), b"\x1b[<64;1;1M");
    }

    /// Asserts that the wheel policy the backend was built with reaches
    /// each pane's terminal, so a configured `lines_per_notch` decides how
    /// many cursor keys one notch sends.
    ///
    /// Case: a user who set `lines_per_notch = 5` opens `less` in a pane
    /// and spins the wheel up one notch.
    #[test]
    fn the_backends_wheel_config_reaches_its_panes_terminal() {
        let mut h = Harness::with_wheel(WheelConfig {
            lines_per_notch: 5,
            ..WheelConfig::default()
        });
        let (root, pane) = h.open_root();
        pane.print(b"\x1b[?1049h");
        h.pump_pane(root);
        h.drain();
        h.send(OrzmuxCommand::Wheel {
            pane: root,
            input: wheel_up(),
        });
        h.settle_writes();
        assert_eq!(pane.received(), b"\x1b[A".repeat(5));
    }

    /// Asserts that a `Wheel` for an unknown pane writes nothing and
    /// produces no event.
    ///
    /// Case: a wheel frame arrives for a pane the user closed a moment
    /// ago.
    #[test]
    fn a_wheel_command_for_an_unknown_pane_is_dropped_silently() {
        let mut h = Harness::new();
        let (_root, root_pane) = h.open_root();
        h.send(OrzmuxCommand::Wheel {
            pane: PaneId(99),
            input: wheel_up(),
        });
        assert!(h.drain().is_empty());
        assert!(root_pane.received().is_empty());
    }

    /// One pointer event on cell (`col`, `row`) with nothing held.
    fn pointer(
        kind: PointerKind,
        button: Option<PointerButton>,
        col: u32,
        row: u32,
        side: CellSide,
    ) -> PointerInput {
        PointerInput {
            kind,
            button,
            cell: CellCoord { col, row },
            side,
            click_count: 1,
            mods: ProtocolModifiers::default(),
        }
    }

    /// Asserts that a `Pointer` press reaches its pane's terminal, which
    /// reports it in that pane's own encoding.
    ///
    /// Case: nvim has turned on button-event tracking and SGR reports in a
    /// pane, and the user clicks in it.
    #[test]
    fn a_pointer_press_reaches_its_panes_terminal() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.print(b"\x1b[?1002h\x1b[?1006h");
        h.pump_pane(root);
        h.drain();
        h.send(OrzmuxCommand::Pointer {
            pane: root,
            input: pointer(
                PointerKind::Press,
                Some(PointerButton::Left),
                3,
                2,
                CellSide::Left,
            ),
        });
        h.settle_writes();
        assert_eq!(pane.received(), b"\x1b[<0;3;2M");
    }

    /// Asserts that a selection drag finished through `Pointer` commands
    /// emits `SelectionCopied` carrying the selected text.
    ///
    /// Case: with no application tracking the mouse, the user drags across
    /// the first word of a line the shell printed and lets go.
    #[test]
    fn a_finished_selection_drag_emits_selection_copied() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.print(b"hello world");
        h.pump_pane(root);
        h.drain();
        h.send(OrzmuxCommand::Pointer {
            pane: root,
            input: pointer(
                PointerKind::Press,
                Some(PointerButton::Left),
                1,
                1,
                CellSide::Left,
            ),
        });
        h.send(OrzmuxCommand::Pointer {
            pane: root,
            input: pointer(PointerKind::Motion, None, 5, 1, CellSide::Right),
        });
        h.send(OrzmuxCommand::Pointer {
            pane: root,
            input: pointer(
                PointerKind::Release,
                Some(PointerButton::Left),
                5,
                1,
                CellSide::Right,
            ),
        });
        let copied: Vec<OrzmuxEvent> = h
            .drain()
            .into_iter()
            .filter(|event| matches!(event, OrzmuxEvent::SelectionCopied { .. }))
            .collect();
        assert_eq!(
            copied,
            vec![OrzmuxEvent::SelectionCopied {
                text: "hello".to_string()
            }]
        );
    }

    /// Asserts that a selection drag released on the right half of the
    /// cell it entered on its left half copies that cell's character too.
    ///
    /// Case: the user drags right across the first word of a line the
    /// shell printed, reaches its last letter on the letter's left half,
    /// and lets go on its right half.
    #[test]
    fn a_drag_released_on_the_right_half_copies_that_character() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.print(b"hello world");
        h.pump_pane(root);
        h.drain();
        for input in [
            pointer(
                PointerKind::Press,
                Some(PointerButton::Left),
                1,
                1,
                CellSide::Left,
            ),
            pointer(PointerKind::Motion, None, 5, 1, CellSide::Left),
            pointer(
                PointerKind::Release,
                Some(PointerButton::Left),
                5,
                1,
                CellSide::Right,
            ),
        ] {
            h.send(OrzmuxCommand::Pointer { pane: root, input });
        }
        let copied: Vec<OrzmuxEvent> = h
            .drain()
            .into_iter()
            .filter(|event| matches!(event, OrzmuxEvent::SelectionCopied { .. }))
            .collect();
        assert_eq!(
            copied,
            vec![OrzmuxEvent::SelectionCopied {
                text: "hello".to_string()
            }]
        );
    }

    /// Asserts that a pointer event for a pane that does not exist is
    /// refused as an unresolved target.
    ///
    /// Case: a click queued for a pane arrives after the pane's shell
    /// exited and the pane closed.
    #[test]
    fn a_pointer_for_an_unknown_pane_is_refused() {
        let mut h = Harness::new();
        h.open_root();
        let result = h.event_loop_mut().backend_mut().pointer(
            PaneId(42),
            pointer(
                PointerKind::Press,
                Some(PointerButton::Left),
                1,
                1,
                CellSide::Left,
            ),
        );
        assert!(
            matches!(result, Err(OrzmuxError::UnresolvedTarget)),
            "{result:?}"
        );
    }

    /// Asserts that `Active` targets resolve in command order, so a kill
    /// queued right after a split removes the new pane.
    ///
    /// Case: the user presses split then kill within one GUI frame,
    /// before any `Layout` has come back.
    #[test]
    fn active_targets_resolve_in_command_order() {
        let mut h = Harness::new();
        let (root, _root_pane) = h.open_root();
        let (new, _new_pane) = split_active(&mut h, 2);
        h.send(OrzmuxCommand::KillPane {
            pane: PaneTarget::Active,
        });
        let events = h.drain();
        assert!(events.iter().any(|e| matches!(
            e,
            OrzmuxEvent::PaneClosed {
                pane,
                reason: CloseReason::Killed
            } if *pane == new
        )));
        assert_eq!(h.backend().tree().panes(), vec![root]);
        assert_eq!(h.backend().tree().active(), Some(root));
    }

    /// Asserts that a kill flushes the pane's pending output before
    /// `PaneClosed`, and that the survivor's resized frame rides in the
    /// following `Layout`.
    ///
    /// Case: the user kills a pane that had just printed something the
    /// coalescer had not yet emitted.
    #[test]
    fn kill_flushes_the_pane_then_closes_and_reflows() {
        let mut h = Harness::new();
        let (root, _root_pane) = h.open_root();
        let (new, new_pane) = split_active(&mut h, 2);
        // NOTE: this pump settles the new pane's bootstrap frame so the
        // write below lands inside an ordinary debounce window instead of
        // being swept into the bootstrap snapshot, which the coalescer
        // always emits on a pane's very first pump regardless of damage.
        h.pump_pane(new);
        h.drain();
        new_pane.print(b"last words");
        h.pump_pane(new);
        h.drain();
        h.send(OrzmuxCommand::KillPane {
            pane: PaneTarget::Id(new),
        });
        let events: Vec<OrzmuxEvent> = h.drain().into_iter().collect();
        let closed_at = events
            .iter()
            .position(|e| matches!(e, OrzmuxEvent::PaneClosed { pane, .. } if *pane == new))
            .expect("PaneClosed");
        let frame_at = events
            .iter()
            .position(|e| matches!(e, OrzmuxEvent::Frame { pane, .. } if *pane == new))
            .expect("a final Frame for the killed pane");
        assert!(frame_at < closed_at, "the final frame precedes PaneClosed");
        let Some(OrzmuxEvent::Layout { layout, frames }) = events.last() else {
            panic!("Layout must be last");
        };
        assert_eq!(layout.panes.len(), 1);
        assert_eq!(
            frames.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
            vec![root]
        );
    }

    /// Asserts that a child exit closes its pane after the remaining
    /// output, and that closing the last pane yields an empty layout.
    ///
    /// Case: the user types `exit` in the only pane.
    #[test]
    fn a_child_exit_closes_the_pane_and_the_last_one_empties_the_layout() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.print(b"logout\r\n");
        pane.exit(Some(0));
        drop(pane);
        h.pump_pane(root);
        let events: Vec<OrzmuxEvent> = h.drain().into_iter().collect();
        assert!(events.iter().any(|e| matches!(
            e,
            OrzmuxEvent::PaneClosed {
                pane,
                reason: CloseReason::ChildExit { code: Some(0) }
            } if *pane == root
        )));
        let Some(OrzmuxEvent::Layout { layout, .. }) = events.last() else {
            panic!("Layout must be last");
        };
        assert!(layout.panes.is_empty());
        assert!(h.backend().tree().is_empty());
    }

    /// Asserts that keyboard input reaches the active pane's PTY and that
    /// a `SelectPane` always answers with a `Layout`, even when refused.
    ///
    /// Case: the user clicks a pane that closed a moment ago, then types.
    #[test]
    fn select_pane_always_answers_with_a_layout_and_keys_reach_the_active_pane() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        let seq = h.send(OrzmuxCommand::SelectPane { pane: PaneId(99) });
        let events = h.drain();
        let Some(OrzmuxEvent::Layout { layout, .. }) = events.front() else {
            panic!("a refused SelectPane still answers with a Layout");
        };
        assert_eq!(layout.seq, seq);
        assert_eq!(layout.active, Some(root));

        h.send(OrzmuxCommand::KeyInput {
            pane: PaneTarget::Active,
            key: TerminalKey::Character(KeyText::new("a").unwrap()),
            mods: TerminalModifiers::default(),
        });
        h.settle_writes();
        assert_eq!(root_pane.received(), b"a");
    }

    /// Asserts that a `KeyInput` for an unknown pane writes nothing and
    /// produces no event, rather than panicking or falling back to
    /// another pane.
    ///
    /// Case: a stale keystroke arrives for a pane the user already closed.
    #[test]
    fn key_input_for_an_unknown_pane_is_dropped_silently() {
        let mut h = Harness::new();
        let (_root, root_pane) = h.open_root();
        h.send(OrzmuxCommand::KeyInput {
            pane: PaneTarget::Id(PaneId(99)),
            key: TerminalKey::Character(KeyText::new("a").unwrap()),
            mods: TerminalModifiers::default(),
        });
        assert!(h.drain().is_empty());
        h.settle_writes();
        assert!(root_pane.received().is_empty());
    }

    /// Asserts that directional selection moves the active pane and
    /// answers with a `Layout` carrying the new active.
    ///
    /// Case: the user presses select-left-pane from the right pane.
    #[test]
    fn select_direction_changes_the_active_pane() {
        let mut h = Harness::new();
        let (root, _root_pane) = h.open_root();
        let (_new, _new_pane) = split_active(&mut h, 2);
        h.send(OrzmuxCommand::SelectPaneDirection {
            direction: PaneDirection::Left,
        });
        let events = h.drain();
        let Some(OrzmuxEvent::Layout { layout, .. }) = events.front() else {
            panic!("expected Layout");
        };
        assert_eq!(layout.active, Some(root));
    }

    /// Asserts that a directional selection with no neighbour in that
    /// direction publishes nothing.
    ///
    /// Case: the user holds select-left with the leftmost pane already
    /// active.
    #[test]
    fn select_direction_into_a_wall_publishes_nothing() {
        let mut h = Harness::new();
        let (_root, _root_pane) = h.open_root();
        h.drain();
        h.send(OrzmuxCommand::SelectPaneDirection {
            direction: PaneDirection::Left,
        });
        assert!(h.drain().is_empty());
    }

    /// Asserts that every `CopySelection` is answered exactly once, with
    /// `None`s when the target cannot be resolved.
    ///
    /// Case: the user presses copy with no selection, and a stale copy
    /// aimed at a pane that no longer exists arrives.
    #[test]
    fn copy_selection_is_always_answered() {
        let mut h = Harness::new();
        let (root, _root_pane) = h.open_root();
        h.send(OrzmuxCommand::CopySelection {
            pane: PaneTarget::Id(root),
        });
        h.send(OrzmuxCommand::CopySelection {
            pane: PaneTarget::Id(PaneId(42)),
        });
        let answers = h
            .drain()
            .into_iter()
            .filter(|event| *event == OrzmuxEvent::SelectionText { text: None })
            .count();
        assert_eq!(answers, 2);
    }

    /// Asserts that a split with no explicit directory starts in the
    /// directory the target pane's shell reported, when the OS reports
    /// no directory for the target's process.
    ///
    /// Case: a shell that reports its directory `cd`s into a project
    /// while the OS cannot be asked for the pane's directory, and the
    /// user splits the pane.
    #[test]
    fn a_split_inherits_the_target_panes_reported_cwd() {
        let project = TempDir::new().expect("a temporary directory");
        let uri = osc7(project.path());
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.print(&uri);
        h.pump_pane(root);
        h.drain();
        assert_eq!(
            h.backend()
                .pane(root)
                .expect("the root pane")
                .cwd()
                .as_deref(),
            Some(project.path())
        );
        h.clear_spawn_cwds();
        split_active(&mut h, 2);
        assert_eq!(h.last_spawn_cwd().as_deref(), Some(project.path()));
    }

    /// Asserts that a split given an explicit directory spawns there
    /// rather than in the target pane's directory.
    ///
    /// Case: a caller opens a split in a directory it names itself while
    /// the target pane's shell has reported another.
    #[test]
    fn an_explicit_cwd_wins_over_the_target_panes_cwd() {
        let project = TempDir::new().expect("a temporary directory");
        let explicit = TempDir::new().expect("a temporary directory");
        let uri = osc7(project.path());
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.print(&uri);
        h.pump_pane(root);
        h.drain();
        h.clear_spawn_cwds();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: Some(explicit.path().to_path_buf()),
            env: vec![],
        });
        h.drain();
        assert_eq!(h.last_spawn_cwd(), Some(explicit.path().to_path_buf()));
    }

    /// Asserts that a pane spawned in an inherited directory passes that
    /// directory on when it is split before its own shell has reported
    /// one and while the OS reports no directory for its process.
    ///
    /// Case: the user splits twice in quick succession while the new
    /// shell has not printed its first prompt and the OS cannot be asked
    /// for the new pane's directory.
    #[test]
    fn a_split_from_a_pane_that_has_not_reported_a_cwd_passes_on_its_spawn_cwd() {
        let project = TempDir::new().expect("a temporary directory");
        let uri = osc7(project.path());
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        pane.print(&uri);
        h.pump_pane(root);
        h.drain();
        split_active(&mut h, 2);
        h.clear_spawn_cwds();
        split_active(&mut h, 3);
        assert_eq!(h.last_spawn_cwd().as_deref(), Some(project.path()));
    }

    /// Asserts that selecting a pane reports focus loss to the pane it
    /// leaves and focus gain to the pane it enters.
    ///
    /// Case: the user moves from one nvim pane to another and back.
    #[test]
    fn select_pane_moves_the_focus_report_from_the_old_pane_to_the_new() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        let (new, new_pane) = split_active(&mut h, 2);
        enable_focus_reporting(&mut h, root, &root_pane);
        enable_focus_reporting(&mut h, new, &new_pane);
        h.send(OrzmuxCommand::SelectPane { pane: root });
        h.send(OrzmuxCommand::SelectPane { pane: new });
        h.settle_writes();
        assert_eq!(root_pane.received(), b"\x1b[I\x1b[O");
        assert_eq!(new_pane.received(), b"\x1b[O\x1b[I");
    }

    /// Asserts that a split reports focus loss to the pane it splits and
    /// nothing to the pane it creates, even when the new pane's unread
    /// start-up output enables focus reporting.
    ///
    /// Case: the user splits the pane running nvim on Windows, where a new
    /// pane's start-up output enables focus reporting.
    #[test]
    fn a_split_reports_focus_loss_to_the_split_pane_only() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        enable_focus_reporting(&mut h, root, &root_pane);
        h.set_spawn_output(b"\x1b[?1004h");
        let (_new, new_pane) = split_active(&mut h, 2);
        h.settle_writes();
        assert_eq!(root_pane.received(), b"\x1b[O");
        assert_eq!(new_pane.received(), b"");
    }

    /// Asserts that a split whose spawn fails reports no focus change.
    ///
    /// Case: the shell binary is missing when the user splits the pane
    /// running nvim.
    #[test]
    fn a_failed_split_reports_no_focus_change() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        enable_focus_reporting(&mut h, root, &root_pane);
        h.fail_next_spawn();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Id(root),
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        assert!(matches!(
            h.drain().front(),
            Some(OrzmuxEvent::SpawnFailed { .. })
        ));
        h.settle_writes();
        assert_eq!(root_pane.received(), b"");
    }

    /// Asserts that killing the active pane reports focus gain to the pane
    /// that becomes active.
    ///
    /// Case: the user closes a scratch pane and lands back in nvim.
    #[test]
    fn killing_the_active_pane_reports_focus_to_the_survivor() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        let (_new, _new_pane) = split_active(&mut h, 2);
        enable_focus_reporting(&mut h, root, &root_pane);
        h.send(OrzmuxCommand::KillPane {
            pane: PaneTarget::Active,
        });
        h.settle_writes();
        assert_eq!(root_pane.received(), b"\x1b[I");
    }

    /// Asserts that window focus changes reach only the active pane, and
    /// only when the focus state changes.
    ///
    /// Case: the user switches from orzma to a browser, the loss is
    /// reported twice, and the user switches back.
    #[test]
    fn window_focus_reports_to_the_active_pane_only_on_change() {
        let mut h = Harness::new();
        let (root, root_pane) = h.open_root();
        let (new, new_pane) = split_active(&mut h, 2);
        enable_focus_reporting(&mut h, root, &root_pane);
        enable_focus_reporting(&mut h, new, &new_pane);
        h.send(OrzmuxCommand::WindowFocus { focused: false });
        h.send(OrzmuxCommand::WindowFocus { focused: false });
        h.send(OrzmuxCommand::WindowFocus { focused: true });
        h.settle_writes();
        assert_eq!(new_pane.received(), b"\x1b[O\x1b[I");
        assert_eq!(root_pane.received(), b"");
    }

    /// Asserts that a failed focus write to the pane being left does not
    /// stop the report to the pane being entered.
    ///
    /// Case: the user moves from a pane whose PTY rejects writes into a
    /// pane running nvim.
    #[test]
    fn a_failed_focus_write_does_not_stop_the_incoming_report() {
        let mut h = Harness::new();
        h.fail_next_writes();
        let (root, root_pane) = h.open_root();
        let (new, new_pane) = split_active(&mut h, 2);
        enable_focus_reporting(&mut h, root, &root_pane);
        enable_focus_reporting(&mut h, new, &new_pane);
        h.send(OrzmuxCommand::SelectPane { pane: root });
        h.settle_writes();
        h.send(OrzmuxCommand::SelectPane { pane: new });
        h.settle_writes();
        assert_eq!(new_pane.received(), b"\x1b[O\x1b[I");
    }

    /// Asserts that a pane whose PTY stops accepting input neither stalls
    /// commands for other panes nor survives `KillPane`.
    ///
    /// Case: the user stops nvim under a debugger in one pane, keeps typing
    /// and pastes into it, then types in the other pane and kills the stuck
    /// one.
    #[test]
    fn a_pane_that_stops_reading_does_not_freeze_the_backend() {
        let gate = BlockingSink::default();
        let thread_gate = gate.clone();
        let (done_tx, done_rx) = bounded::<(Vec<u8>, bool)>(1);
        thread::spawn(move || {
            let mut h = Harness::new();
            let stuck_writer = thread_gate.clone();
            h.block_next_writes(thread_gate);
            let (stuck, _stuck_pane) = h.open_root();
            let (other, other_pane) = split_active(&mut h, 2);
            for text in ["x", "y"] {
                h.send(OrzmuxCommand::KeyInput {
                    pane: PaneTarget::Id(stuck),
                    key: TerminalKey::Character(KeyText::new(text).unwrap()),
                    mods: TerminalModifiers::default(),
                });
            }
            h.send(OrzmuxCommand::Paste {
                pane: PaneTarget::Id(stuck),
                text: "a pasted line".to_string(),
            });
            stuck_writer.wait_until_writing();
            h.send(OrzmuxCommand::KeyInput {
                pane: PaneTarget::Id(other),
                key: TerminalKey::Character(KeyText::new("z").unwrap()),
                mods: TerminalModifiers::default(),
            });
            h.backend()
                .pane(other)
                .expect("the other pane")
                .tty
                .settle_writes();
            let other_received = other_pane.received();
            h.send(OrzmuxCommand::KillPane {
                pane: PaneTarget::Id(stuck),
            });
            let _ = done_tx.send((other_received, h.backend().pane(stuck).is_none()));
        });
        let outcome = done_rx.recv_timeout(Duration::from_secs(10));
        gate.release();
        let (other_received, stuck_closed) = match outcome {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => {
                panic!("the backend froze on a pane that stopped reading stdin")
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("the harness thread panicked before reporting")
            }
        };
        assert_eq!(other_received, b"z");
        assert!(stuck_closed);
    }

    /// Asserts that a resize publishes one layout whose divider moved
    /// and repaints only the panes whose size changed.
    ///
    /// Case: the user drags the divider of a two-pane window to the
    /// right, widening the left pane.
    #[test]
    fn a_resize_publishes_a_moved_layout_and_repaints_the_resized_panes() {
        let mut h = Harness::new();
        let (_root, _pane) = h.open_root();
        h.send(OrzmuxCommand::NewPane {
            request: RequestId(2),
            at: NewPaneAt::Split {
                pane: PaneTarget::Active,
                orientation: SplitOrientation::Vertical,
            },
            cwd: None,
            env: vec![],
        });
        let mut opened = h.drain();
        let Some(OrzmuxEvent::Layout { layout, .. }) = opened.pop_back() else {
            panic!("expected a Layout after the split");
        };
        let split = layout.separators[0].split;

        h.send(OrzmuxCommand::ResizeSplit {
            split,
            position: 60,
        });
        let mut events = h.drain();

        let Some(OrzmuxEvent::Layout { layout, frames }) = events.pop_back() else {
            panic!("expected a Layout event");
        };
        assert_eq!(layout.separators[0].x, 60);
        assert_eq!(frames.len(), 2);
    }

    /// Asserts that a resize naming a split the tree does not have
    /// publishes nothing at all.
    ///
    /// Case: the pane the pointer was resizing closed a frame earlier,
    /// so the drag's next command names a divider that is gone.
    #[test]
    fn a_resize_of_an_unknown_split_publishes_nothing() {
        let mut h = Harness::new();
        let (_root, _pane) = h.open_root();
        h.drain();

        h.send(OrzmuxCommand::ResizeSplit {
            split: SplitId(999),
            position: 60,
        });

        assert!(h.drain().is_empty());
    }

    /// Asserts that a pane frame emitted when its coalescer deadline passes
    /// wakes the GUI exactly once, and that a flush with nothing queued
    /// does not wake it.
    ///
    /// Case: a background build prints one line and goes quiet while the
    /// user is looking at another application.
    #[test]
    fn a_deadline_frame_wakes_the_gui_and_an_empty_flush_does_not() {
        let mut h = Harness::new();
        let (root, pane) = h.open_root();
        h.pump_pane(root);
        h.drain();
        let before = h.wake_count();
        h.drain();
        assert_eq!(h.wake_count(), before, "an empty flush sends no wake");
        pane.print(b"hello");
        h.pump_pane(root);
        assert!(h.drain().is_empty(), "the pump alone emits no frame");
        thread::sleep(Duration::from_millis(15));
        h.service_deadlines();
        let events = h.drain();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OrzmuxEvent::Frame { pane, .. } if *pane == root))
        );
        assert_eq!(h.wake_count(), before + 1);
    }
}
