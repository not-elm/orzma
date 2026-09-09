# orzmux buffer saturation: measurement record

Status: measured on 2026-09-10; PR 4 does not ship on this evidence
(see Decision). Spec:
`docs/superpowers/specs/2026-09-09-orzmux-buffer-saturation-design.md`
(§3.2 is the protocol this file records).

The results below decide PR 4, frame coalescing. PRs 2 and 3 ship
regardless.

## How to run

Build and run the app with the queue logging on. Every other target
stays at its default level.

```
RUST_LOG=orzmux::queues=debug cargo run
```

The three log lines to watch, all under the `orzmux::queues` target at
`debug`:

| Line | Fields | Source |
| --- | --- | --- |
| `chunk queue peak` | `pane`, `depth` | backend, once a second per pane, only when the depth exceeded 1 |
| `event and command queue peaks` | `events`, `commands` | backend, once a second, only when either exceeded 1 |
| `drained more frames than the threshold in one update` | `frames`, `elapsed` | GUI drain, every `Update` that applied more than 8 frames |

`depth` counts unread reader chunks of up to 4 KiB each. `events` and
`commands` are the lengths of the backend-to-GUI and GUI-to-backend
channels. `frames` counts standalone frames plus the frames bundled in
layouts. `elapsed` times only the drain loop, not the observers that
apply the frames, so treat it as informational.

## Load cases

Run each case for at least ten seconds and note the largest value of
each field seen during the run.

1. `cat` on a file of at least 100 MiB. For example:
   `head -c 100m /dev/urandom | base64 > /tmp/big.txt && cat /tmp/big.txt`
2. `yes`
3. Holding a key down while continuously live-resizing the window.
4. A CEF webview pane rendering alongside case 1.

## Results

Fill in one row per case. Leave a cell as `none` when the line never
appeared.

All rows: 2026-09-10, release build of the full stack at `4ff6c97`
(PR 1 + PR 2 + PR 3), macOS 26.6.2 on an Apple M4 Pro. `none` means
the line never appeared.

| Case | Peak chunk depth | Peak event depth | Peak command depth | Largest frames per `Update` | Its drain time |
| --- | --- | --- | --- | --- | --- |
| 1. `cat` 100 MiB (two runs, ~15 s and ~9 s) | 256, held at the cap for every sampled second | 3 | 0 | none | none |
| 2. `yes` | not isolated in the log | | | | |
| 3. key held during live resize | not isolated in the log; no command backlog was seen during any load | | | | |
| 4. `orzbrowser` on YouTube beside `cat` 100 MiB (~13 s) | 256 in the `cat` pane, 3 in the browser pane | 2 | 0 | none | none |
| 5. `cargo build --release` inside the terminal (~5 min, PR 1 build) | 5 | 8 | 0 | 9 | 17.75 µs |

Reading the chunk depth: 256 is `Pty::CHUNK_QUEUE_CAPACITY`, so the
reader parked for the whole `cat` and the child's `write(2)` blocked
on the kernel PTY buffer. The VT parser, not the reader, is the
bottleneck on a 100 MiB stream, and the bound is what keeps that
backlog at 1 MiB instead of growing without limit as it would on PR 1
alone.

Reading the event depth: a steady 2 to 3 is one frame plus whatever
else was queued when the backend sampled. The GUI drained every frame
within the `Update` it arrived in; the only drain over the threshold
was one 9-frame hiccup in five minutes of compiler output, roughly a
30 to 100 ms GUI stall.

## Decision

PR 4 ships if any case logs a drain of more than 8 frames in one
`Update`.

2026-09-10: PR 4 does not ship. Cases 1 and 4 never produced a drain
above the threshold, and the one 9-frame drain in case 5 is a single
brief hiccup whose cost (nine observer dispatches and nine
`runs_to_cells` passes) is not worth a coalescer. Path B never backed
up even with a CEF webview painting beside a saturated `cat`. Path A
saturates on `cat`, which PR 2 now bounds; that is the defect this
work set out to fix.

Case 3 (a held key during a continuous live resize) was not isolated
in the logs. It is the one scenario that could still change this
decision, because a live resize can block the winit event loop and
queue frames behind it. Reopen the decision only if a run of case 3
logs a drain of more than 8 frames.
