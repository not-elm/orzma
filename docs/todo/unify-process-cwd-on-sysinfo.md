# `process_cwd.rs` を全プラットフォームで `sysinfo` に統一する

由来: Windows の CWD 継承対応（`docs/superpowers/specs/2026-09-20-windows-split-pane-cwd-design.md` §7）で
Windows arm のみ `sysinfo` を採用したため、macOS / Linux との実装方法が不揃いになっている。

## 置き換えられるもの

| 現行 | `sysinfo` での代替 | 効果 |
|---|---|---|
| macOS `read_cwd` = `proc_pidinfo(PROC_PIDVNODEPATHINFO)` + `MaybeUninit` + NUL 走査（約 40 行 `unsafe`） | `Process::cwd()` | `unsafe` 1 ブロック削除 |
| macOS `wrapper_children` = `proc_listchildpids`（約 12 行 `unsafe`） | `processes()` を `Process::parent()` でフィルタ | `unsafe` 1 ブロック削除 |
| Linux `read_cwd` = `read_link("/proc/{pid}/cwd")`（1 行、`unsafe` なし） | `Process::cwd()`（中身は `realpath("/proc/<pid>/cwd")`） | 効果なし |

`sysinfo` の macOS 実装は `proc_pidinfo(PROC_PIDVNODEPATHINFO)` + `pvi_cdir` であり、
現行コードと同一の API を使っている（`sysinfo-0.38.4/src/unix/apple/macos/process.rs:471-495`）。

## 置き換えられないもの

- `master.process_group_leader()` — PTY 由来。`portable_pty` のまま。
- `enterable_cwd` の検証（`path.join(".").is_dir()`）— `sysinfo` は検証しない。

## 代償

1. 呼び出しごとに全プロセス列挙になる。`ProcessesToUpdate::Some(&[pid])` はフィルタするだけで
   列挙を省略しない（macOS / Linux / Windows いずれも同じ構造）。現行は候補 pid を直接叩くだけ。
2. `orzma_tty` は現在 macOS で `libc` のみ依存、Linux は依存ゼロ。全面置き換えると全プラットフォームで
   `sysinfo` が乗る。
3. macOS / Linux は動作中でテストもあるため、純粋なリグレッションリスクになる。
4. `read_cwd` の失敗理由が失われる。`sysinfo` は `Option` しか返さない。

## 判断

Windows arm が先に `sysinfo` になるため、移行の足場はできる。
利益（`unsafe` 2 ブロック削除と cfg ツリーの解消）と代償の交換になるので、
Windows 対応とは独立に判断する。
