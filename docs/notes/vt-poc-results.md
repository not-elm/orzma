# Phase 1 PoC Results

Empirical findings from the Phase 1 (Foundation) PoC tests that drive a real
PTY through `TerminalService` and inspect the resulting `alacritty_terminal`
state. Test source: `daemon/terminal/tests/vt_integration.rs`. Reproduce via:

```bash
cargo test -p ozmux_terminal --features test-helpers --test vt_integration \
  -- --nocapture --test-threads=1
```

Tested against `alacritty_terminal = "0.26"` and `vte = "0.15"` on macOS
(Darwin 25.3.0).

## Task 17: `TermDamage` API behavior (alacritty 0.26)

`Term::damage()` returns `TermDamage::Full | TermDamage::Partial(iter)`. The
`Partial` iterator yields `LineDamageBounds` items (one per damaged line in
the viewport). `Term::reset_damage()` clears the tracker.

Observed for a freshly-spawned 80x24 PTY running `sh`, after one
`echo line1; echo line2\n` write and a 200 ms settle:

| Phase                                | Variant observed                  |
| ------------------------------------ | --------------------------------- |
| After first write (Phase A)          | `TermDamage::Full`                |
| After `reset_damage()`, no new input | `TermDamage::Partial { line_count: 1 }` |

Takeaways for Phase 2:

- The first paint after spawn comes through as `Full`. The Phase 2 delta
  encoder needs a "full snapshot" code path for this case anyway (initial
  WS frame); the damage API maps naturally onto it.
- `reset_damage()` is not idempotent in the no-input case: it returns to
  `Partial` with a small line count rather than an empty `Partial(0)` or
  another `Full`. The single damaged line is most likely the cursor row
  (cursor blink / position bookkeeping inside alacritty). Phase 2's delta
  encoder must therefore treat "Partial with N lines" as the steady state
  and tolerate empty-payload deltas without emitting noise frames.
- The damage tracker is per-screen (alt vs primary). Phase 2 must call
  `reset_damage()` immediately after each frame emission, under the same
  short `VtState` lock the bridge task already takes for `Processor::
  advance`.

## Task 18: Alt-screen escape support (alacritty 0.26)

DEC private modes that *should* toggle the alternate screen buffer:

| Escape  | `?Nh` enters ALT_SCREEN | `?Nl` exits to primary | Notes                                                 |
| ------- | ----------------------- | ---------------------- | ----------------------------------------------------- |
| `?1049` | yes                     | yes                    | The xterm-modern variant. Save cursor + alt + clear.  |
| `?1047` | **no**                  | n/a (never entered)    | Not mapped in `vte 0.15`; ignored by alacritty 0.26.  |
| `?47`   | **no**                  | n/a (never entered)    | Not mapped in `vte 0.15`; ignored by alacritty 0.26.  |

Verified at the parser layer: `vte::ansi` only maps `1049` to
`NamedPrivateMode::SwapScreenAndSetRestoreCursor`; codes `47` and `1047`
fall through as unrecognized private modes and are silently dropped.

Implications for Phase 2:

- It is safe to rely exclusively on `?1049` for alt-screen detection in the
  damage / event pipeline. Modern TUIs (vim, htop, less, tmux) emit `?1049`
  by default. The few legacy programs that still emit `?1047` or `?47`
  will not switch buffers under our VT, which matches what plain alacritty
  does — i.e., it is not an ozmux regression and does not need a custom
  shim.
- The PoC test for `?1049` doubles as a regression guard against a future
  alacritty/vte bump quietly removing alt-screen support.

## Test-harness note

PTYs are in cooked mode by default: writing raw `\x1b[?1049h` bytes to the
master writer goes through the line discipline and gets echoed back as
printable `^[[?1049h` rather than as a real escape sequence. The PoC tests
work around this by asking the shell to emit the escape itself
(`printf '\033[?1049h'`), which produces the sequence on the slave-write
side and the master reader picks it up verbatim. Phase 2's bridge consumes
real-program output and is not affected — this caveat applies only when a
test wants to *inject* an escape.
