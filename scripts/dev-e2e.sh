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
cmd_start() {
  handle_existing_pid_file
  require_free_port "${VITE_PORT}"
  require_free_port "${DAEMON_PORT}"
  echo "start: pre-flight ok (servers not yet implemented)" >&2
  exit 1
}
cmd_stop()  { echo "stop: not yet implemented" >&2;  exit 1; }

port_in_use() {
  # Returns 0 (success) when the TCP port is being listened on.
  local port="$1"
  lsof -nP -iTCP:"${port}" -sTCP:LISTEN >/dev/null 2>&1
}

require_free_port() {
  local port="$1"
  if port_in_use "$port"; then
    echo "error: port ${port} is already in use." >&2
    echo "       inspect with: lsof -nP -iTCP:${port} -sTCP:LISTEN" >&2
    exit 1
  fi
}

pid_alive() {
  # Returns 0 if the given PID belongs to a live process.
  local pid="$1"
  [[ -n "${pid}" ]] && kill -0 "${pid}" 2>/dev/null
}

handle_existing_pid_file() {
  # If a PID file exists, decide whether it is stale (delete it) or live (refuse).
  [[ -f "${PID_FILE}" ]] || return 0
  local any_alive=0
  while IFS= read -r pid; do
    [[ -z "${pid}" ]] && continue
    if pid_alive "${pid}"; then
      any_alive=1
    fi
  done < "${PID_FILE}"
  if [[ "${any_alive}" -eq 1 ]]; then
    echo "error: harness already running (see ${PID_FILE})." >&2
    echo "       run: make dev-e2e:stop" >&2
    exit 1
  fi
  echo "info: removing stale ${PID_FILE}" >&2
  rm -f "${PID_FILE}"
}

main "$@"
