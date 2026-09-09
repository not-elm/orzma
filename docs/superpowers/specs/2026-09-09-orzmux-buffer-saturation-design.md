# orzmux buffer saturation — design

Date: 2026-09-09 / branch: `fix-orzmux-buffer-saturation` / base: `84b77ab`
Source notes: `docs/todo/orzmux-buffer-saturation.md`

## 1. Problem

Terminal output crosses three unbounded crossbeam channels on its way to
the GPU:

| | Path | Definition |
| --- | --- | --- |
| A | PTY reader thread → `OrzmaTty` | `crates/orzma_tty/src/pty.rs:101` |
| B | backend → GUI (`OrzmuxEvent`) | `crates/orzmux/src/client.rs:48` |
| C | GUI → backend (`OrzmuxCommand`) | `crates/orzmux/src/client.rs:47` |

Path A is the prime suspect. The reader thread sends every 4 KiB read
without ever blocking, so when VT parsing falls behind the PTY the queue
grows without bound, and the back-pressure the kernel PTY buffer would
otherwise apply to the child's `write(2)` is lost in userspace.

Path B does not grow in the steady state (the coalescer bounds how long
a pane's pending output waits, 3 ms idle or 12 ms at most, and the GUI
drains everything every `Update`), but it grows while the GUI stalls
(live resize, heavy CEF paint, GPU starvation). Because a `Frame` is a
diff, every queued frame must be applied in order when the GUI returns.
The GPU rebuild is already gated once per `Update` on
`Changed<TerminalGrid>`, so what accumulates is one observer dispatch
and one `runs_to_cells` pass per queued frame, not a repaint per frame.
The memory those frames occupy while the GUI is stalled is not
recoverable by anything on the GUI side.

Which path actually overflows is unmeasured. This design instruments
both, restores back-pressure on A, cuts a per-row allocation that
amplifies the cost of every queued frame, and specifies frame
coalescing for B whose PR ships only if measurement shows B growing.

## 2. Scope

In scope, as four ordered PRs:

1. **(a) Instrumentation.** Sampled queue-depth logging in the backend
   plus a frames-drained count and drain time in the GUI drain.
2. **(b) Bounded chunk channel.** Path A becomes `bounded(256)`.
3. **(e) Row allocation.** `Row::to_runs` stops reserving one `Run` per
   column.
4. **(c) Frame coalescing.** `BitOrAssign for Frame` in `orzma_vt` plus a
   `FrameCoalescer` in the `bevy_orzmux` drain. Gated on (a).

Out of scope: a bounded or high-water-marked event channel (the doc's
(d)), a configurable bound, backend-side merging, and any change to
`orzmux::protocol`.

## 3. (a) Instrumentation

### 3.1 Design

The backend thread can see all three queues, so it is the single
sampling point. A new file `crates/orzmux/src/backend/queue_sample.rs`
defines:

```rust
/// How many reader chunks wait unparsed in one pane's chunk channel
/// (path A), in units of one `read(2)` result of up to 4 KiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ChunkDepth(pub usize);

/// Tracks per-queue peaks between samples and hands them out at most
/// once per `SAMPLE_INTERVAL`.
pub struct QueueSampler { .. }

impl QueueSampler {
    pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
    pub fn new(now: Instant) -> Self;
    /// Records one pane's chunk depth, retaining the maximum since the
    /// last sample.
    pub fn record_pane_depth(&mut self, pane: PaneId, depth: ChunkDepth);
    /// Records the event and command channel depths, retaining each
    /// maximum since the last sample.
    pub fn record_channel_depths(&mut self, events: usize, commands: usize);
    /// Returns the peaks and resets them once `SAMPLE_INTERVAL` has
    /// elapsed since the last sample and any peak is non-zero.
    pub fn sample(&mut self, now: Instant) -> Option<QueueSample>;
    /// When an unreported non-zero peak exists, the instant the next
    /// sample is due; `None` otherwise.
    pub fn report_deadline(&self) -> Option<Instant>;
}

/// The peaks one sample reports.
pub struct QueueSample {
    pub chunks: Vec<(PaneId, ChunkDepth)>,
    pub events: usize,
    pub commands: usize,
}
```

`Backend` owns one `QueueSampler`. Recording happens at the top of
each `Backend::run` iteration, immediately after `wait_ready` returns
and before `pump_pane` or `drain_commands` runs, because a pane wake
drains up to `PUMP_ROUNDS × MAX_CHUNKS_PER_PUMP` (256, the whole
bounded capacity) chunks before anything after the pump could look:

- for each pane,
  `record_pane_depth(id, ChunkDepth(pane.tty.pending_chunk_count()))`,
  where `pending_chunk_count` is a new `OrzmaTty` accessor wrapping
  `Pty::chunk_receiver().len()`; the newtype lives in `orzmux` so
  `orzma_tty` keeps returning a plain `usize`;
- `record_channel_depths(self.events.len(), self.commands.len())`, the
  backend's `Sender` end of B and `Receiver` end of C.

After `service_deadlines`, `Backend::run` calls `sample(now)` and logs a
returned sample inline with `tracing::debug!` under the target
`orzmux::queues`: one line per pane whose chunk peak is non-zero and one
line for B and C when either peak is non-zero. `wait_ready` folds
`report_deadline()` into its deadline so a backend that goes idle with
an unreported peak wakes once to report it; an idle terminal with no
peak logs nothing and adds no wake. Keeping the decision (`sample`)
apart from the effect (the log call) keeps the sampler testable without
capturing log output.

Each recorded depth is one `len()` load per queue, a lock-free head and
tail read that retries only when the tail moved between the two loads,
and the per-pane fold allocates only when a pane's first non-zero depth
after a sample grows the peak list; the sampler is therefore always
compiled in, and `RUST_LOG=orzmux::queues=debug` turns the output on.

Depths are recorded peaks, not high-water marks: a queue that fills and
drains between two recordings is not seen. Sampling before the pump
makes that window one `Select` wake, which is as tight as a sampling
design gets without instrumenting the reader thread.

### 3.2 Measurement protocol

The backend cannot see path B grow while it is blocked in `wait_ready`,
and event depth mixes frames with layouts and signals, so the GUI drain
carries the second half of the instrumentation: `drain_orzmux_events`
counts the frames it drained in one `Update` (standalone `Frame` events
plus the frames bundled in `Layout`s) and times the drain, and logs both
under the same `orzmux::queues` target at `debug` when the count
exceeds 8. That is the exact moment a stalled GUI resumes, which is the
scenario (c) targets.

After PR (a) lands, run each load case for at least ten seconds with
`RUST_LOG=orzmux::queues=debug` and record, in
`docs/todo/orzmux-buffer-saturation.md` §7, the peak chunk depth, event
depth, and command depth from the backend, and the largest frames
drained per `Update` with its drain time from the GUI:

1. `cat` on a file of at least 100 MiB.
2. `yes`.
3. Holding a key down during a continuous live window resize.
4. A CEF webview pane rendering alongside case 1.

The (c) PR ships if any case logs a drain of more than 8 frames whose
wall time exceeds 16 ms, one GUI frame budget. The event-depth peak
stays informational: (c) merges frames only after the GUI wakes, so it
cannot shrink the queue that builds during the stall, only the work of
draining it. The chunk depth (A) result is informational too; PR (b)
ships regardless because the missing back-pressure is a defect
independent of measured growth.

## 4. (b) Bounded chunk channel

### 4.1 Design

`Pty` gains:

```rust
/// How many reader chunks may wait unparsed before the reader thread
/// parks: 256 × 4 KiB = 1 MiB per pane, matching Alacritty's ceiling.
pub const CHUNK_QUEUE_CAPACITY: usize = 256;
```

`Pty::spawn` creates the chunk channel with
`bounded::<Vec<u8>>(Self::CHUNK_QUEUE_CAPACITY)`. Nothing else in the
constructor changes, and the test-only constructors (`with_master`,
`with_master_and_channels`) keep their own channels, so no test fixture
is affected.

End-to-end effect: the reader thread's `send` parks when the queue is
full, so it stops reading; the kernel PTY buffer fills; the child's
`write(2)` blocks. Pane teardown drops the receiver, which makes the
parked `send` return `Err` and ends the thread, so teardown cannot hang
on a parked reader. `Backend::wait_ready` only calls `select.recv` on
the receiver, which is unchanged for a bounded channel, and
`OrzmaTty::pump` keeps its `MAX_CHUNKS_PER_PUMP` budget.

The workspace stays on crossbeam-channel 0.5.15 or later (the lock pins
0.5.15): 0.5.12 through 0.5.14 carry RUSTSEC-2025-0024, a double free in
the unbounded flavor that paths B and C still use.

The blast radius is `crates/orzma_tty/src/pty.rs` (plus the
`pending_chunk_count` accessor from §3, which lands in PR (a)).

### 4.2 Read loop extraction

The body of the reader thread's loop moves into a free function shared
by both platform variants:

```rust
/// What the reader thread reports about its progress, shared with
/// whoever needs to know whether output is still flowing.
struct ReaderProgress {
    /// When the reader last completed a read or a send.
    last_activity: Mutex<Instant>,
    /// `true` from the moment `try_send` finds the queue full until
    /// the blocking `send` returns.
    parked: AtomicBool,
}

/// Reads `reader` to EOF or error, sending each read as one chunk.
/// Returns when the reader ends or the receiver is gone.
fn forward_chunks(
    reader: &mut dyn Read,
    chunk_tx: &Sender<Vec<u8>>,
    progress: &ReaderProgress,
);
```

`forward_chunks` stamps `last_activity` after every completed read and
again after every `send` returns, and sets `parked` only after
`try_send` reports a full queue, holds it until the blocking `send`
returns, and stamps the returned send before clearing it. Both
`spawn_reader_thread` variants share one `Arc<ReaderProgress>` with the
thread; only the Windows exit watcher (§4.3) reads it today. The
function carries no platform code, and the extraction exists so the
parking behavior can be tested with an in-memory `Read` and no PTY,
against the production progress type.

### 4.3 Windows exit-watcher guard

The ConPTY exit watcher reports the child's exit once `OUTPUT_QUIESCENCE`
(50 ms) has passed since the reader's last completed read. A reader
parked on a full queue also completes no reads, so without a guard the
watcher can report the exit while output is still in the pipe, and
`OrzmaTty::pump` would then close the pane the moment the queue is
momentarily empty, losing the tail of the child's output.

The watcher reads the reader's `ReaderProgress` (§4.2) instead of a
bare timestamp. Quiescence is "not parked and no activity for
`OUTPUT_QUIESCENCE`", where activity is a completed read or a returned
`send`. Stamping after the send closes the race where a send that
parked longer than 50 ms returns with a stale read stamp and the watcher
fires before the next read completes.

The existing `OUTPUT_QUIESCENCE_CAP` (2 s) exists so a pseudoconsole
that streams forever cannot delay the exit indefinitely. A parked reader
is not streaming, so the cap counts only time spent not parked: the
watcher accumulates unparked time and returns when that reaches the cap.
A reader parked for longer than 2 s behind a slow parser therefore still
delays the exit, which is the loss the guard exists to prevent.

The poll loop sleeps `OUTPUT_QUIESCENCE.saturating_sub(idle)` with a
10 ms floor, because once the loop keeps waiting past the idle window
`idle` exceeds `OUTPUT_QUIESCENCE` and the current unchecked subtraction
would panic on underflow.

The window between unpark and the next completed read is bounded by one
read latency, the same tolerance the 50 ms window already grants the
reader today; it is not zero.

## 5. (e) Row allocation

`Row::to_runs` (`crates/orzma_vt/src/screen/grid/row.rs:41`) replaces
`Vec::with_capacity(self.0.len())` with
`Vec::with_capacity(self.0.len().min(RUNS_RESERVE))`, where
`RUNS_RESERVE` is a private constant of 8. A single-attribute row then
carries capacity for 8 runs instead of one per column, and a
syntax-highlighted wide row with 20 to 40 runs reaches its size in one
to three reallocations instead of five or six from an empty vector.
Geometric growth bounds the final capacity at twice the run count.
Output is unchanged. This lands as its own PR because it is independent
of every other change.

## 6. (c) Frame coalescing

### 6.1 `BitOrAssign for Frame` (orzma_vt)

An in-place merge on the type that owns the frame contract, expressed
as the `|=` operator and declared in `crates/orzma_vt/src/frame.rs`:

```rust
/// `older |= newer` merges `newer` into `older` so that applying the
/// result equals applying `older` and then `newer`.
impl BitOrAssign for Frame {
    fn bitor_assign(&mut self, newer: Frame);
}
```

The operator reads as "overlay `newer` on `older`": `rows` is a union
where the right-hand side wins, the changed-only sections take the
right-hand `Some`, and `hyperlinks` is a set union, which is what `|`
conventionally means for sets. The trait impl satisfies the
associated-function rule in `rust.md`, and the doc comment sits on the
`impl` block because a trait method cannot carry a public doc of its
own that rustdoc surfaces on the type.

Per-field rules:

| Field | Rule |
| --- | --- |
| `size`, `cursor`, `display_offset`, `vi_cursor`, `selection` | Take `newer`'s value. |
| `rows` | Union keyed by `line`; where both carry a line, `newer` wins. Output stays ascending by line. Any row whose line is at or beyond `newer.size.rows` is dropped. |
| `placements`, `palette` | `newer.field.or(self.field)`: the newest `Some` wins, `None` means unchanged. |
| `hyperlinks` | Append `newer`'s entries whose id `self` does not already carry. |

Two fast paths precede the merge: an empty `newer.rows` (a state-only
frame carrying a cursor or selection change) leaves `self.rows`
untouched, and a `newer` that is a full repaint (`rows.len() ==
usize::from(size.rows)`, which every basis change produces) moves
`newer.rows` in outright. Otherwise both lists are ascending, so the
union is a linear two-pointer merge into a fresh `Vec` with capacity
`self.rows.len() + newer.rows.len()` at most.

`TerminalGrid::apply` is itself incremental, so `|=` has a precise
oracle: for any two frames, applying `a` then `b` to a grid must leave
it equal to applying `a` after `a |= b`. That equivalence is what the
renderer-side test in §7 pins.

Invariant recorded in the doc comment: the emitter stages
`DamageSpan::Full` on every resize (`device.rs:80`) and every viewport
motion (`device.rs:120`), so a frame whose `size` or `display_offset`
differs from its predecessor carries every viewport row. Rows from the
older frame therefore never survive under a different basis; `|=`
does not need to compare bases, only lines.

### 6.2 `FrameCoalescer` (bevy_orzmux)

A new file `crates/bevy_orzmux/src/drain/coalesce.rs` (with `drain.rs`
declaring `pub mod coalesce;`) defines a Bevy-free helper:

```rust
/// Merges consecutive frames per pane until a non-frame event or the
/// end of the drain flushes them.
pub struct FrameCoalescer {
    /// The pending frame per pane, in first-seen pane order.
    pub pending: Vec<(PaneId, Frame)>,
}

impl FrameCoalescer {
    /// A coalescer with nothing pending.
    pub fn new() -> Self;
    /// Overlays `frame` onto the pane's pending frame with `|=`, or
    /// starts one.
    pub fn push(&mut self, pane: PaneId, frame: Frame);
    /// Hands back every pending frame in first-seen pane order.
    pub fn take(&mut self) -> Vec<(PaneId, Frame)>;
}
```

`pending` is a `Vec` with a linear lookup because the pane count is
small; no map type is introduced.

### 6.3 Drain loop

`drain_orzmux_events` builds one `FrameCoalescer` per call (nothing is
carried across `Update`s, because every `Update` still drains the whole
queue) and processes events as follows:

| Event | Handling |
| --- | --- |
| `Frame { pane, frame }` | `coalescer.push(pane, frame)`. |
| `Layout { layout, frames }` | Flush before the empty-layout check that triggers `OrzmuxSessionEnded`, apply the layout as today, then `push` each bundled frame so it merges with following `Frame` events. |
| Every other event | Flush, then `apply_event` as today. |
| Queue empty | Flush, before the disconnect check that may trigger `OrzmuxSessionEnded`. |

Flush means: for each `(pane, frame)` from `coalescer.take()`, call
`trigger_frame` exactly as the current code does for one frame.

Ordering guarantees that follow from the table:

- A frame never overtakes or falls behind a `PaneOpened`, `PaneClosed`,
  `Signal`, `SelectionText`, or `Layout`, because each of those flushes
  first.
- A pane's last frame reaches its entity before `PaneClosed` despawns it.
- A pane's last frame precedes `OrzmuxSessionEnded`, whether the session
  ends through an empty layout or a disconnect, because both flushes in
  the table run before the respective check.
- A `Layout`'s bundled frames are ordered after that layout, as today.
- Frames of different panes are independent, so their relative order
  across panes carries no meaning and is not preserved beyond first-seen
  order.

Cost per `Update` becomes one `|=` per queued frame plus one
`TtyFrameSignal` and one `TerminalGrid::apply` per pane per
uninterrupted run of `Frame` events, instead of one signal and one apply
per queued frame. The saving is observer dispatches and repeated
`runs_to_cells` passes over rows that several frames repainted; the GPU
rebuild was already once per `Update`, and the memory queued during the
stall is unchanged.

## 7. Testing

All tests run without a PTY or GPU unless stated.

**orzma_tty**

- `forward_chunks` with a 2 MiB in-memory reader and a `bounded(256)`
  channel: the thread produces exactly 256 chunks and parks; draining the
  receiver lets it finish, and the total bytes forwarded equal the input.
- Dropping the receiver while the reader is parked ends the thread.
- (Windows only) `wait_for_output_quiescence` does not return while
  `parked` is `true`, even after the idle window has elapsed and even
  past `OUTPUT_QUIESCENCE_CAP`.
- (Windows only) the unpark transition: `parked` clears while the read
  stamp is older than the idle window, and the watcher still waits for
  the send stamp plus a fresh idle window before returning.
- (Windows only) `OUTPUT_QUIESCENCE_CAP` still bounds an unparked reader
  that never goes idle.

**orzma_vt**

- `|=`: newer row wins on a shared line; an older-only row survives;
  `palette: None` in `newer` keeps the older `Some`; `placements:
  Some(vec![])` in `newer` overrides an older non-empty `Some`;
  hyperlinks with a duplicate id are not appended twice; rows at or
  beyond a shrunk `newer.size.rows` are dropped; a full-repaint `newer`
  replaces every row; an empty `newer.rows` keeps every older row;
  `size`, `cursor`, `display_offset`, `vi_cursor`, and `selection` come
  from `newer`.
- `to_runs`: a single-attribute row's run vector has capacity of at most
  `RUNS_RESERVE`, well below its column count.

**orzma_tty_renderer**

- The `|=` oracle: for a handful of hand-built frame pairs (partial
  then partial on overlapping and disjoint lines, partial then full
  repaint, full repaint then partial, a palette change followed by a
  frame with `palette: None`), `grid.apply(a); grid.apply(b)` leaves
  the grid equal to `grid.apply(&{ a |= b; a })` on a fresh grid.

**orzmux**

- `QueueSampler`: two `record_pane_depth` calls within one interval keep the
  higher depth; `sample` returns `None` before the interval elapses and
  `Some` carrying the peaks after it; after a sample the peaks start
  over; `sample` returns `None` when every peak is zero;
  `report_deadline` is `Some` only while an unreported non-zero peak
  exists. Tests assert on the returned `QueueSample`, never on captured
  log output.
- `Backend::wait_ready` wakes at `report_deadline` when no pane deadline
  is earlier.

**bevy_orzmux**

- The drain counts standalone and bundled frames in one `Update` and
  reports the count with the drain time when it exceeds 8; a drain of 8
  or fewer reports nothing. The count is computed by a helper the test
  calls directly, so no log capture is needed.

- Two `Frame`s for one pane in one drain produce one `TtyFrameSignal`
  whose rows are the merged set.
- `Frame`, `Signal`, `Frame` for one pane produce two frame signals with
  the signal between them, checked through an ordered log in the test's
  `Seen` resource.
- A `Layout` with a bundled frame followed by a `Frame` event for the
  same pane produces one frame signal after the layout change.
- A `Frame` followed by `PaneClosed` delivers the frame to the entity
  before it is despawned.
- Existing drain tests keep passing unchanged.

## 8. PR staging

| PR | Content | Crates |
| --- | --- | --- |
| 1 | (a) `OrzmaTty::pending_chunk_count`, `QueueSampler`, wiring in `Backend::run` and `wait_ready`, drain frame count and timing | orzma_tty, orzmux, bevy_orzmux |
| 2 | (b) `CHUNK_QUEUE_CAPACITY`, `ReaderProgress`, `forward_chunks`, Windows watcher guard | orzma_tty |
| 3 | (e) `to_runs` allocation | orzma_vt |
| 4 | (c) `BitOrAssign for Frame`, `FrameCoalescer`, drain loop | orzma_vt, bevy_orzmux |

PR 4 opens only after the §3.2 measurements are recorded and show B
growing. PRs 1 to 3 are independent of that result.

## 9. Risks

- **A stalled parser now stalls the child.** That is the intended
  back-pressure, but a bug that stops `pump` from being called (for
  example a `Select` that never reports the pane ready) would now freeze
  the child instead of growing memory. The existing `wait_ready` tests
  cover readiness; the instrumentation makes a stuck queue visible.
- **The Windows guard changes exit timing** only when the reader is
  parked, which is the case it exists for. Normal exits are unaffected,
  and an unparked reader that never goes idle is still cut off by
  `OUTPUT_QUIESCENCE_CAP`. A reader parked indefinitely behind a
  consumer that never drains would delay the exit indefinitely, but such
  a consumer is a torn-down pane, whose dropped receiver unparks the
  reader with an error.
- **Sampling is best-effort.** The backend records once per wake, so a
  queue that fills and drains inside one pump is invisible to it; the
  drain-side frame count is exact but only for path B.
- **`|=` trusts the full-repaint invariant.** If a future change
  stops staging `DamageSpan::Full` on a viewport motion, merged frames
  could carry rows from two bases. The invariant is pinned by
  `device.rs` tests today and restated in the `BitOrAssign` impl's doc
  comment.
