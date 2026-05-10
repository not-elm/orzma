#!/usr/bin/env bash
# scripts/dev-e2e.sh — lifecycle for the Playwright UI verification harness.
# Subcommands:
#   setup  — one-time prerequisites (pnpm install, cargo build, browser install)
#   start  — launch vite (5173) + daemon (3200) in the background, wait for ready
#   stop   — kill the running harness via .ozmux/e2e.pid
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PID_FILE="${REPO_ROOT}/.ozmux/e2e.pid"
VITE_PORT=5173
DAEMON_PORT=3200
READY_TIMEOUT_SECONDS=30

usage() {
  echo "usage: $0 {setup|start|stop}" >&2
  exit 64
}

main() {
  [[ $# -eq 1 ]] || usage
  case "$1" in
    setup) cmd_setup ;;
    start) cmd_start ;;
    stop)  cmd_stop ;;
    *)     usage ;;
  esac
}

cmd_setup() { echo "setup: not yet implemented" >&2; exit 1; }
cmd_start() { echo "start: not yet implemented" >&2; exit 1; }
cmd_stop()  { echo "stop: not yet implemented" >&2;  exit 1; }

main "$@"
