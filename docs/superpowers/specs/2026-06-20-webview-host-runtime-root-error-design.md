# webview_host: type `RuntimeRoot` resolution errors with `thiserror`

Date: 2026-06-20
Crate: `crates/webview_host` (package `ozmux_webview_host`)

## Goal

Add `thiserror` to `crates/webview_host` and use it to give
`RuntimeRoot::resolve_in` a typed error that distinguishes a real I/O
failure from the logical "socket path too long" failure. Today both
collapse into a single `std::io::Result`, with the logical case
shoehorned into `std::io::Error::other(format!(...))`.

## Background / problem

`host.rs::RuntimeRoot::resolve_in` can fail two ways:

1. **I/O failure** from `std::fs::create_dir_all` / `set_permissions`,
   propagated with `?` as `std::io::Error`.
2. **Logical failure**: the longest socket filename a webview uses
   (`<name>.handlers.sock`) under the chosen parent would overflow the
   platform `sun_path` limit (`SUN_PATH_MAX` = 104 on macOS, 108
   elsewhere). This is currently returned as
   `Err(std::io::Error::other(format!("'{name}' socket path exceeds {SUN_PATH_MAX} bytes")))`
   — a string packed into an `io::Error`, indistinguishable by type from
   a genuine I/O error.

Callers that want to react differently to "the environment is too deep"
versus "the filesystem rejected the operation" cannot, because both are
the same `io::Error`.

## Blast radius (verified)

- **Only one caller** of `RuntimeRoot::resolve_in`:
  `src/control_plane.rs:338`. It consumes the error with
  `Err(e) => tracing::error!(error = %e, "control-plane runtime dir failed")`,
  i.e. via `Display` only. `thiserror` derives `Display`, so the caller
  compiles and behaves unchanged.
- **Existing tests** in `host.rs` call `RuntimeRoot::resolve_in(...).unwrap()`.
  `unwrap()` needs `Debug`, which the new enum derives, so they compile
  unchanged.
- The workspace root `Cargo.toml` already declares
  `thiserror = { version = "2" }` under `[workspace.dependencies]`; five
  other crates use `thiserror = { workspace = true }`. This change only
  adds the same one-line reference to `webview_host`.

## Design (approach A)

### 1. Dependency

`crates/webview_host/Cargo.toml`, under `[dependencies]`:

```toml
thiserror = { workspace = true }
```

### 2. Error type (`host.rs`)

```rust
/// Error returned when resolving a [`RuntimeRoot`].
#[derive(Debug, thiserror::Error)]
pub enum RuntimeRootError {
    #[error("'{name}' socket path exceeds {limit} bytes")]
    SocketPathTooLong { name: String, limit: usize },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

- `limit` is a field (not hardcoded) because `SUN_PATH_MAX` is
  platform-dependent; carrying it keeps the displayed message identical
  to today's per-platform value.
- `Io` is `#[error(transparent)]` so an underlying I/O error renders
  exactly as it does now. The sole caller already logs its own context
  string (`"control-plane runtime dir failed"`), so wrapping the I/O
  error with an extra message would be redundant.
- The enum, its variants, and their fields all get `///` doc comments,
  matching the existing `crates/ozmux_configs/src/error.rs` style (which
  documents variants and fields). `#[from] std::io::Error` is written
  inline (no `use std::io;`), also matching that file.

### 3. Signature + body changes (`host.rs`)

- `resolve_in` return type: `std::io::Result<Self>` →
  `Result<Self, RuntimeRootError>`.
- `new_in` return type: `std::io::Result<Self>` →
  `Result<Self, RuntimeRootError>`. The `?` operators on `create_dir_all`
  / `set_permissions` auto-convert `io::Error` via `#[from]`. Keeping
  both functions on the same return type avoids wrapping the two
  `return Self::new_in(...)` sites in `resolve_in`.
- Replace the `Err(std::io::Error::other(format!(...)))` line with:

  ```rust
  Err(RuntimeRootError::SocketPathTooLong {
      name: name.to_owned(),
      limit: SUN_PATH_MAX,
  })
  ```

### 4. Re-export policy (`lib.rs`)

No change. `RuntimeRoot` itself is reached as
`ozmux_webview_host::host::RuntimeRoot` (it is not re-exported from
`lib.rs`). For consistency, `RuntimeRootError` is likewise left in the
`host` module (`host::RuntimeRootError`) and not re-exported. The sole
caller never names the type, so no re-export is needed.

### 5. Test addition (`host.rs`, `#[cfg(test)] mod tests`)

Add one test that proves the logical failure now surfaces as a typed
variant rather than a generic I/O error. Use a `name` long enough that
even the `/tmp/ozmux-webview` fallback overflows `SUN_PATH_MAX` (the
fixed path contributes the name twice — `<name>/sock/<name>.handlers.sock`
— so a ~60-char name forces the overflow on both macOS and Linux), and
assert:

```rust
assert!(matches!(
    RuntimeRoot::resolve_in(parent.path(), 1, &long_name),
    Err(RuntimeRootError::SocketPathTooLong { .. })
));
```

The existing `runtime_root_falls_back_to_tmp_when_too_long` test stays as
is (it exercises the *success* fallback path).

## Out of scope (YAGNI)

- `asset.rs` / `dyn_scheme.rs` error handling
  (`AssetOutcome::NotFound`, `Result<_, u16>`) is left untouched.
- No new `lib.rs` re-exports beyond what already exists.
- No context wrapping on the `Io` variant.

## Verification

- `cargo build -p ozmux_webview_host`
- `cargo test -p ozmux_webview_host`
- `cargo build` (workspace, ensures the `src/control_plane.rs` caller
  still compiles against the new return type)
- `cargo clippy --workspace` / `cargo fmt`
