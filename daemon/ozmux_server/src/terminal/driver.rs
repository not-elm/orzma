//! Per-surface VT/PTY driver: owns one terminal's `(Pty, Vt)` on a dedicated
//! OS thread and multiplexes PTY output, child exit, client commands, and the
//! coalescer deadline via `crossbeam::Select`.
