# Per-Process CEF Profile Directory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each ozmux process a unique per-PID CEF `root_cache_path` so multiple instances run concurrently without Chromium's per-profile singleton-lock error.

**Architecture:** A new `src/cef_profile.rs` module owns a `CefProfileDir` RAII guard — a per-PID directory `$TMPDIR/ozmux-cef/<pid>/` created at startup (after sweeping dead-owner dirs), passed to bevy_cef's `CefPlugin.root_cache_path`, and removed on drop. The startup sweep (not `Drop`) is the primary cleanup, since macOS teardown may not unwind. It mirrors the existing `RuntimeRoot` idiom in `crates/webview_host/src/host.rs`.

**Tech Stack:** Rust (edition 2024), Bevy 0.18, `bevy_cef` (git `passthrough`, CEF 145), `libc` (already a dependency) for PID-liveness, `std::fs` / `std::env::temp_dir`.

**Spec:** `docs/superpowers/specs/2026-06-20-cef-per-process-profile-design.md`

## Global Constraints

- Edition 2024, toolchain pinned `1.95` (`rust-toolchain.toml`).
- No `mod.rs`; module files are `foo.rs` (`.claude/rules/rust.md`).
- All `use` at file top in one contiguous block; no inline fully-qualified paths in signatures/bodies.
- Comments: only `// TODO:` / `// NOTE:` / `// SAFETY:`. `// NOTE:` is for critical caveats only.
- Doc comments: `//!` on the module file; `///` on every externally-`pub` item (not required for `pub(crate)`).
- Visibility: start private; widen only for real callers. Items used only across modules in this binary crate are `pub(crate)`, not `pub`.
- Item ordering: `pub` / `pub(crate)` items before private (no-modifier) items.
- Parameter ordering: mutable params before immutable (n/a here — all params are immutable by-value).
- `unsafe { ... }` requires a `// SAFETY:` comment.
- Lint/format gate before commit: `cargo clippy --workspace --all-targets` clean and `cargo fmt`.
- Target platform is macOS; `libc::kill` covers all unix, with a `#[cfg(not(unix))]` fallback.

## File Structure

| File | Responsibility |
| --- | --- |
| `src/cef_profile.rs` (**new**) | `CefProfileDir` RAII guard: per-PID profile dir, startup sweep of dead-owner dirs, `Drop` cleanup, PID-liveness. Pure `std::fs` + `libc::kill`; unit-tested without GUI/CEF. |
| `src/webview_render.rs` (**modify**) | `cef_plugin()` gains a `root_cache_path: PathBuf` parameter and sets `CefPlugin.root_cache_path: Some(..)`. |
| `src/main.rs` (**modify**) | Declare `mod cef_profile;`; in `main()`, `CefProfileDir::acquire()` before building the app, pass its path into `cef_plugin(..)`, hold the guard for the app's lifetime. |

This is one cohesive change (a new leaf module plus two wiring edits). It is a single task: the module is dead code without the wiring, so a reviewer gates them together. Steps follow TDD; the final committed state is warning-clean.

---

### Task 1: Per-process CEF profile directory

**Files:**
- Create: `src/cef_profile.rs`
- Test: `src/cef_profile.rs` (inline `#[cfg(test)] mod tests`)
- Modify: `src/main.rs` (mod declaration + `main()` wiring)
- Modify: `src/webview_render.rs` (`cef_plugin` signature + body)

**Interfaces:**
- Produces:
  - `pub(crate) struct CefProfileDir` with `pub(crate) fn acquire() -> std::io::Result<CefProfileDir>` and `pub(crate) fn path(&self) -> &std::path::Path`.
  - Private internals (same module + tests only): `fn resolve_in(parent: &Path, pid: u32) -> std::io::Result<CefProfileDir>`, `fn sweep_in(base: &Path, is_alive: impl Fn(u32) -> bool, self_pid: u32)`, `fn pid_alive(pid: u32) -> bool`.
  - `cef_plugin(dyn_registry: DynAssetRegistry, root_cache_path: PathBuf) -> CefPlugin` (changed signature).
- Consumes: bevy_cef `CefPlugin { root_cache_path: Option<String>, .. }` (`/Users/taiga/workspace/bevy_cef/wt/passthrough/src/lib.rs:48-58`); `RuntimeRoot` idiom for reference (`crates/webview_host/src/host.rs`).

---

- [ ] **Step 1: Write the failing tests + module skeleton**

Create `src/cef_profile.rs` with the module doc, the public surface as stubs (so the test module compiles and the asserts fail), and the four hermetic unit tests:

```rust
//! Per-process CEF profile directory: a unique `root_cache_path` per ozmux
//! instance so concurrent instances never collide on Chromium's per-profile
//! singleton lock.

use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// A per-process CEF profile directory (`$TMPDIR/ozmux-cef/<pid>/`), removed on drop.
///
/// Chromium's `ProcessSingleton` permits only one live process per profile
/// directory, so a shared profile makes a second ozmux instance fail. Keying the
/// directory by PID guarantees concurrent instances never collide, since live
/// PIDs are unique.
pub(crate) struct CefProfileDir {
    path: PathBuf,
}

impl CefProfileDir {
    /// Sweeps stale per-PID profile directories (dead owners) under the shared
    /// base, then creates and claims this process's own profile directory.
    pub(crate) fn acquire() -> std::io::Result<Self> {
        todo!()
    }

    /// The absolute path to pass to CEF as `root_cache_path`.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn resolve_in(_parent: &Path, _pid: u32) -> std::io::Result<Self> {
        todo!()
    }
}

impl Drop for CefProfileDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn sweep_in(_base: &Path, _is_alive: impl Fn(u32) -> bool, _self_pid: u32) {
    todo!()
}

#[cfg(unix)]
fn pid_alive(_pid: u32) -> bool {
    todo!()
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_in_creates_0700_dir_and_drops() {
        let parent = tempfile::tempdir().unwrap();
        let path = {
            let profile = CefProfileDir::resolve_in(parent.path(), 4242).unwrap();
            assert!(profile.path().is_absolute());
            assert_eq!(profile.path(), parent.path().join("4242"));
            #[cfg(unix)]
            {
                let mode = std::fs::metadata(profile.path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o700);
            }
            profile.path().to_path_buf()
        };
        assert!(!path.exists(), "Drop must remove the profile dir");
    }

    #[test]
    fn resolve_in_replaces_stale_same_pid_dir() {
        let parent = tempfile::tempdir().unwrap();
        let stale = parent.path().join("4243");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("SingletonLock"), b"stale").unwrap();

        let profile = CefProfileDir::resolve_in(parent.path(), 4243).unwrap();

        assert_eq!(profile.path(), stale);
        assert!(profile.path().exists());
        assert!(
            !profile.path().join("SingletonLock").exists(),
            "a fresh profile dir must not inherit the stale lock marker"
        );
        assert!(
            std::fs::read_dir(profile.path()).unwrap().next().is_none(),
            "the re-created profile dir must be empty"
        );
    }

    #[test]
    fn sweep_in_removes_dead_keeps_alive_and_self() {
        let base = tempfile::tempdir().unwrap();
        for pid in ["100", "200", "300"] {
            std::fs::create_dir_all(base.path().join(pid)).unwrap();
        }
        let is_alive = |pid: u32| pid == 100;

        sweep_in(base.path(), is_alive, 300);

        assert!(base.path().join("100").exists(), "alive owner kept");
        assert!(!base.path().join("200").exists(), "dead owner swept");
        assert!(base.path().join("300").exists(), "self never swept");
    }

    #[test]
    fn sweep_in_ignores_non_numeric_entries() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("not-a-pid")).unwrap();
        std::fs::write(base.path().join("README"), b"x").unwrap();
        std::fs::create_dir_all(base.path().join("200")).unwrap();
        let is_alive = |_pid: u32| false;

        sweep_in(base.path(), is_alive, 999);

        assert!(base.path().join("not-a-pid").exists(), "non-numeric dir untouched");
        assert!(base.path().join("README").exists(), "stray file untouched");
        assert!(!base.path().join("200").exists(), "numeric dead owner swept");
    }
}
```

Then declare the module in `src/main.rs`. Add `mod cef_profile;` to the module-declaration block (alphabetical, between `mod bootstrap;` and `mod configs;`):

```rust
mod bootstrap;
mod cef_profile;
mod configs;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --bin ozmux-gui cef_profile -- --nocapture`
Expected: the three tests that exercise `resolve_in` / `sweep_in` FAIL via `todo!()` panic (`not yet implemented`). (`resolve_in_creates_0700_dir_and_drops`, `resolve_in_replaces_stale_same_pid_dir`, `sweep_in_removes_dead_keeps_alive_and_self`, `sweep_in_ignores_non_numeric_entries` all panic.)

- [ ] **Step 3: Implement `resolve_in`, `sweep_in`, `acquire`, `pid_alive`**

Replace the three `todo!()` bodies (`acquire`, `resolve_in`, `sweep_in`, and the unix `pid_alive`) with the real implementations:

```rust
    pub(crate) fn acquire() -> std::io::Result<Self> {
        let base = std::env::temp_dir().join("ozmux-cef");
        std::fs::create_dir_all(&base)?;
        #[cfg(unix)]
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))?;
        let pid = std::process::id();
        sweep_in(&base, pid_alive, pid);
        Self::resolve_in(&base, pid)
    }
```

```rust
    fn resolve_in(parent: &Path, pid: u32) -> std::io::Result<Self> {
        let path = parent.join(pid.to_string());
        // NOTE: no concurrent process can share our PID, so a pre-existing dir
        // here is a stale leftover from a dead same-PID process; removing it
        // keeps the profile freshly ephemeral. It must never inherit cross-run
        // state — a reused stale SingletonLock would otherwise mislead Chromium.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self { path })
    }
```

```rust
fn sweep_in(base: &Path, is_alive: impl Fn(u32) -> bool, self_pid: u32) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != self_pid && !is_alive(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
```

```rust
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: `kill` with signal 0 sends no signal; it performs only the
    // existence/permission check and has no preconditions on `pid`.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    // NOTE: ESRCH means no such process (dead); any other errno (e.g. EPERM —
    // the process exists but is owned by another user) means alive.
    // Misclassifying a live PID as dead would let the sweep delete a running
    // instance's profile directory.
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --bin ozmux-gui cef_profile`
Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 5: Wire `cef_plugin` and `main()`**

In `src/webview_render.rs`, add the import (top `use` block) and change `cef_plugin`:

```rust
use std::path::PathBuf;
```

```rust
/// Builds the `CefPlugin` with the `ozma-dyn://` (dynamic, Tier 1) scheme bound
/// to its shared `DynAssetRegistry`, using `root_cache_path` as this process's
/// unique CEF profile directory (one Chromium singleton lock per instance).
pub fn cef_plugin(dyn_registry: DynAssetRegistry, root_cache_path: PathBuf) -> CefPlugin {
    CefPlugin {
        custom_schemes: vec![custom_dyn_scheme(dyn_registry)],
        command_line_config: cef_command_line_config(),
        root_cache_path: Some(root_cache_path.to_string_lossy().into_owned()),
        ..Default::default()
    }
}
```

In `src/main.rs`, add the import to the `use` block:

```rust
use crate::cef_profile::CefProfileDir;
```

In `main()`, acquire the profile right after `let dyn_registry = DynAssetRegistry::default();` and bind it so it lives for the whole app run:

```rust
    let dyn_registry = DynAssetRegistry::default();
    let cef_profile =
        CefProfileDir::acquire().expect("create per-process CEF profile directory");
```

Then pass the path into `cef_plugin` (replace the existing `cef_plugin(dyn_registry.clone()),` line inside the first `.add_plugins((..))` tuple):

```rust
            cef_plugin(dyn_registry.clone(), cef_profile.path().to_path_buf()),
```

The `cef_profile` binding stays in scope until `main()` returns (after `.run()`), so its `Drop` is the best-effort secondary cleanup; the startup sweep is primary.

- [ ] **Step 6: Build, lint, and re-test (final state must be warning-clean)**

Run: `cargo build --bin ozmux-gui`
Expected: compiles with no warnings (no `dead_code` for `CefProfileDir` / `acquire` / `path`, now that `main()` uses them).

Run: `cargo clippy --workspace --all-targets`
Expected: no warnings.

Run: `cargo fmt`
Then: `cargo test --bin ozmux-gui cef_profile`
Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 7: Commit**

```bash
git add src/cef_profile.rs src/main.rs src/webview_render.rs
git commit -m "$(cat <<'EOF'
fix(cef): per-process root_cache_path so multiple instances run concurrently

Each ozmux process now gets a unique CEF profile dir ($TMPDIR/ozmux-cef/<pid>),
keyed by PID, so concurrent instances never collide on Chromium's per-profile
singleton lock. A startup sweep removes dead-owner dirs (primary cleanup, since
macOS teardown may not unwind); Drop is best-effort secondary cleanup.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 8: Manual verification (cannot be automated — needs GUI + CEF)**

This step does not commit; it confirms the runtime fix. Requires `make setup-cef` to have been run.

1. Build and launch two instances:
   - Terminal A: `cargo run` — confirm the embedded webview renders.
   - Terminal B (while A is running): `cargo run` — confirm it ALSO launches and renders, with **no** CEF singleton/profile error.
2. While both run: `ls $TMPDIR/ozmux-cef/` shows one `<pid>` directory per live instance.
3. Quit both. Launch a third instance, then quit it; confirm `ls $TMPDIR/ozmux-cef/` does not accumulate dead-owner dirs (the startup sweep reclaimed them).
4. Confirm ozmux no longer writes a fresh lock into `~/Library/Application Support/CEF/User Data` (its `SingletonLock` is no longer (re)created by ozmux launches).

---

## Self-Review

**1. Spec coverage:**
- Per-PID unique `root_cache_path` → Step 5 (`cef_plugin` sets `Some(path)`), Step 3/5 (`acquire` builds `$TMPDIR/ozmux-cef/<pid>`). ✓
- Startup sweep of dead-owner dirs (primary cleanup) → Step 3 `sweep_in` + `acquire`; tested Step 1/4. ✓
- `Drop` secondary cleanup → Step 1 `impl Drop`; tested by `resolve_in_creates_0700_dir_and_drops`. ✓
- Same-PID stale-dir removal (fresh ephemeral invariant) → Step 3 `resolve_in` `remove_dir_all`; tested by `resolve_in_replaces_stale_same_pid_dir`. ✓
- Base dir `0700` + per-PID dir `0700` → Step 3 `acquire` / `resolve_in`; tested by `resolve_in_creates_0700_dir_and_drops`. ✓
- `libc::kill(pid, 0)` liveness (ESRCH=dead, EPERM/0=alive) + non-unix fallback → Step 3 `pid_alive` + Step 1 `#[cfg(not(unix))]`. ✓
- Incognito/in-memory comes free (bevy_cef leaves `cache_path` empty) → no code needed; documented in spec. ✓
- Hold guard for app lifetime → Step 5 `cef_profile` binding in `main()`. ✓
- Manual two-instance verification → Step 8. ✓

**2. Placeholder scan:** No `TBD`/`TODO`/"handle errors" placeholders; every code step shows complete code. The `todo!()` bodies in Step 1 are intentional TDD red-state stubs, fully replaced in Step 3. ✓

**3. Type consistency:** `CefProfileDir`, `acquire() -> io::Result<Self>`, `path(&self) -> &Path`, `resolve_in(&Path, u32) -> io::Result<Self>`, `sweep_in(&Path, impl Fn(u32)->bool, u32)`, `pid_alive(u32) -> bool`, and `cef_plugin(DynAssetRegistry, PathBuf) -> CefPlugin` are used identically across Steps 1, 3, and 5. ✓
