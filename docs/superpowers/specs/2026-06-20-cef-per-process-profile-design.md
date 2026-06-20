# Per-Process CEF Profile Directory — Design

**Date:** 2026-06-20
**Status:** Draft (pending spec review)
**Topic:** Fix the CEF error that occurs when multiple ozmux instances run concurrently.

## Problem

Launching a second ozmux instance while a first is already running produces a
CEF error and the second instance's embedded webview fails to start.

### Root cause (confirmed)

ozmux never sets a per-instance CEF `root_cache_path`, so every instance shares
one Chromium profile directory, and Chromium permits only one live process per
profile directory.

The chain:

1. `src/webview_render.rs` — `cef_plugin()` builds
   `CefPlugin { custom_schemes, command_line_config, ..Default::default() }`.
   The `..Default::default()` leaves **`root_cache_path: None`**.
2. That flows through bevy_cef's `MessageLoopPlugin` → `cef_initialize`, which
   does `root_cache_path.unwrap_or_default()` → CEF `Settings.root_cache_path = ""`
   (empty). `cache_path` is also empty (bevy_cef never sets it).
3. With **both** `cache_path` and `root_cache_path` empty, CEF on macOS falls
   back to the fixed platform default profile directory:
   `~/Library/Application Support/CEF/User Data`.
4. Confirmed live on this machine: that directory exists and holds Chromium's
   process-singleton lock:

   ```
   SingletonLock   -> <host>-<pid>
   SingletonCookie -> <number>
   SingletonSocket -> /var/folders/.../SingletonSocket
   ```

5. Chromium's `ProcessSingleton` permits exactly one process per profile dir.
   A second ozmux points at the **same** `User Data` dir, cannot acquire the
   lock held by instance #1, and the CEF init / browser creation fails.

### Secondary findings (context, not in scope to fix here)

- bevy_cef's doc comment on `CefPlugin::root_cache_path` claims empty defaults to
  "the executable's directory"; the real CEF behavior (proven by the live lock
  dir above) is the platform default user-data dir. The doc is inaccurate.
  Fixing it is an optional upstream follow-up in the separate bevy_cef repo.
- The repo previously isolated this in the old out-of-process browser host
  (git `3c30383`, "per-activity profile isolation"); the current in-process
  bevy_cef path dropped explicit cache scoping.
- `RuntimeRoot` (the per-PID control-socket dir in `crates/webview_host`) is
  already isolated per process, but it is unrelated to the CEF cache and does
  not help here.

## Goal & non-goals

**Goal:** Multiple ozmux instances run concurrently, each with its own CEF
profile, with no singleton-lock conflict.

**Chosen model:** Ephemeral per-process profile. Each instance gets a unique,
absolute `root_cache_path` keyed by PID. Two concurrent instances always have
distinct PIDs, so they can never collide on the lock. No cross-run persistence
is expected (the Tier-1 inline webviews are app-driven and ephemeral).

**Non-goals (YAGNI):**

- No persistent cache across runs; no cross-run instance identity.
- No config knob for the profile location.
- Not fixing bevy_cef's inaccurate doc comment in this change.
- Windows is not a target (ozmux ships macOS-only per `CLAUDE.md` / `Makefile`).

## Design

A new `src/cef_profile.rs` module owns one concern: compute, create, and clean
up a unique per-process CEF profile directory. It mirrors the existing
`RuntimeRoot` (`crates/webview_host/src/host.rs`) idiom: a per-PID directory
under the system temp dir, `0700`, removed on `Drop`, with a testable
`resolve_in(parent, pid)` core.

### Data flow

```
main()
  └─ CefProfileDir::acquire()
        ├─ sweep dead-owner dirs under $TMPDIR/ozmux-cef/
        └─ create $TMPDIR/ozmux-cef/<pid>/   (0700)
  └─ cef_plugin(dyn_registry, profile.path())   // passes Some(path) as root_cache_path
        └─ CEF: unique profile → unique SingletonLock → no cross-instance conflict
  └─ guard `cef_profile` held in main() for the app's lifetime
        └─ Drop = best-effort secondary cleanup
```

Because bevy_cef leaves `cache_path` empty, CEF runs the profile in **incognito
(in-memory) mode**: page caches are in-memory and no profile-specific data is
persisted. Installation-specific data (the `SingletonLock` / `SingletonCookie`
/ `SingletonSocket` skeleton) is still written under `root_cache_path` — which
is exactly what we want, since the per-PID lock is the whole point. No change to
bevy_cef is required; we only populate the `root_cache_path` it already exposes.
Note bevy_cef's `cef_initialize` already `create_dir_all`s a non-empty
`root_cache_path`, so ozmux's own directory creation (below) is partly redundant
— but still required, because the sweep and the `0700` chmod must happen *before*
CEF initializes, and CEF never chmods.

### The `CefProfileDir` type

```rust
//! Per-process CEF profile directory: a unique root_cache_path per ozmux
//! instance so concurrent instances never collide on Chromium's per-profile
//! singleton lock.

/// A per-process CEF profile directory (`<base>/ozmux-cef/<pid>/`), removed on drop.
pub struct CefProfileDir {
    path: PathBuf,
}

impl CefProfileDir {
    /// Sweeps stale per-PID profile dirs (dead owners) under the shared base,
    /// then creates and claims this process's own profile dir.
    pub fn acquire() -> std::io::Result<Self> {
        // base = std::env::temp_dir().join("ozmux-cef")
        // create base (chmod 0700, parity with RuntimeRoot's intermediate dir);
        // sweep_in(base, pid_alive, std::process::id()); resolve_in(base, pid)
    }

    /// The absolute path to pass as CEF `root_cache_path`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    // --- testable internals ---

    fn resolve_in(parent: &Path, pid: u32) -> std::io::Result<Self> {
        // root = parent/<pid>
        // remove_dir_all(root) first — no concurrent process can share our PID,
        //   so any pre-existing root is a stale leftover from a dead same-PID
        //   process; removing it guarantees a fresh, truly-ephemeral profile.
        // create_dir_all(root); chmod 0700 (parity with RuntimeRoot)
    }

    fn sweep_in(base: &Path, is_alive: impl Fn(u32) -> bool, self_pid: u32) {
        // for each dir entry whose name parses as u32 `n`:
        //   if n != self_pid && !is_alive(n) { let _ = remove_dir_all(entry); }
        // ignore non-numeric entries and all errors (best-effort)
    }
}

impl Drop for CefProfileDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // libc::kill(pid as i32, 0): 0 or EPERM => alive; ESRCH => dead
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    // conservative: never sweep on non-unix; rely on Drop only
    true
}
```

`resolve_in` and `sweep_in` take injected parameters (a parent path; a liveness
predicate) so they are unit-testable with no GUI, no CEF, and no real PIDs —
exactly the `RuntimeRoot::resolve_in` testability shape already in the repo.

The root crate already depends on `libc` (`Cargo.toml`), so `pid_alive` adds no
new dependency.

### Wiring

- `cef_plugin` gains a parameter:
  `pub fn cef_plugin(dyn_registry: DynAssetRegistry, root_cache_path: PathBuf) -> CefPlugin`.
  It sets `root_cache_path: Some(root_cache_path.to_string_lossy().into_owned())`
  instead of relying on `..Default::default()`'s `None`.
- `main()` resolves the profile before building the app and holds the guard for
  the app's lifetime:

  ```rust
  let cef_profile = CefProfileDir::acquire().expect("create CEF profile dir");
  // ...
  cef_plugin(dyn_registry.clone(), cef_profile.path().to_path_buf())
  // ...
  // `cef_profile` stays bound in main() across `.run()`.
  ```

### Cleanup semantics (robustness core)

- **Primary = startup sweep.** `Drop` is **not** guaranteed to run on every exit
  path: the macOS winit/CEF teardown may tear the process down without unwinding
  `main()`'s stack, and bevy_cef's helper-process path calls `std::process::exit`
  (which skips destructors). Rather than depend on resolving exactly which quit
  paths unwind, the design makes the sweep — not `Drop` — the primary cleanup.
  The sweep removes any `ozmux-cef/<pid>` whose owner is dead, reclaiming dirs
  left by `kill -9`, hard exits, or any non-unwinding teardown.
- **Secondary = `Drop`** on clean teardown.
- The sweep **never** deletes the current PID's dir or a live PID's dir.
- PID reuse — **other** PIDs: if a dead instance's PID now belongs to a live
  unrelated process, the sweep skips that stale dir (treats it as alive). This is
  a tiny, bounded leak that self-heals when that PID later dies, and is harmless
  because no live ozmux uses that stale dir. (Even if such a stale dir were
  reused, its `SingletonLock` points at a dead PID, which Chromium breaks
  automatically.)
- PID reuse — **our own** PID: a leftover `ozmux-cef/<our-pid>` can exist at
  startup if a previous process with the same PID died without cleanup. Since no
  *concurrent* process can share our PID, that dir is necessarily stale, so
  `resolve_in` deletes it before recreating — preserving the "fresh, ephemeral
  profile per run" invariant (no inherited cross-run state).

### Error handling

- `acquire()` returns `io::Result`; `main()` `.expect()`s it with a clear
  message. If the temp dir is not writable, CEF could not run anyway, so failing
  fast is consistent with bevy_cef's own `cef_initialize` assertion.
- `sweep_in` is best-effort: all errors (unreadable entries, races with another
  instance sweeping the same base) are ignored.

## Testing

### Unit tests (pure fs, no GUI/CEF) — in `src/cef_profile.rs`

- `resolve_in` creates `<parent>/<pid>`, `path()` is absolute, mode is `0700`;
  `Drop` removes the directory.
- `resolve_in` on a `<parent>/<pid>` that already exists (with a stale marker
  file inside) replaces it with a fresh empty directory — proves the same-PID
  stale-dir guarantee.
- `sweep_in` with an injected liveness predicate over pids `{alive, dead, self}`
  removes only the dead one and keeps the alive one and `self`.
- `sweep_in` ignores non-numeric directory entries and stray files (does not
  panic, does not delete them).

### Manual verification (cannot be automated — needs GUI + CEF)

- Launch two ozmux instances → both render webviews, no CEF error.
- Confirm `$TMPDIR/ozmux-cef/<pid>` directories appear per instance and stale
  ones are swept on a later launch.
- Confirm ozmux no longer writes a fresh lock into
  `~/Library/Application Support/CEF/User Data`.

## Files touched

- **New:** `src/cef_profile.rs` (`CefProfileDir`, `pid_alive`, tests).
- **Modified:** `src/main.rs` — declare `mod cef_profile;`, resolve the guard,
  pass the path into `cef_plugin`.
- **Modified:** `src/webview_render.rs` — `cef_plugin` takes a `PathBuf` and sets
  `root_cache_path: Some(..)`.
