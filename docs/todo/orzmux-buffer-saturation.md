# orzmux buffer saturation: measurement record

Status: waiting for measurements. Spec:
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

| Case | Peak chunk depth | Peak event depth | Peak command depth | Largest frames per `Update` | Its drain time | Date, build |
| --- | --- | --- | --- | --- | --- | --- |
| 1. `cat` 100 MiB | | | | | | |
| 2. `yes` | | | | | | |
| 3. key held during live resize | | | | | | |
| 4. webview beside case 1 | | | | | | |

## Decision

PR 4 ships if any case logs a drain of more than 8 frames in one
`Update`. Record the decision here with the date once the table is
filled in.
