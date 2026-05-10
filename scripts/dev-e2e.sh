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

cmd_setup() { echo "setup: not yet implemented" >&2; return 1; }
cmd_start() {
  handle_existing_pid_file
  require_free_port "${VITE_PORT}"
  require_free_port "${DAEMON_PORT}"

  mkdir -p "$(dirname "${PID_FILE}")"

  local log_dir="${REPO_ROOT}/.ozmux/logs"
  mkdir -p "${log_dir}"
  local vite_log="${log_dir}/vite.log"
  local daemon_log="${log_dir}/daemon.log"
  : > "${vite_log}"
  : > "${daemon_log}"

  # Enable job control so each `&` job becomes its own process group leader.
  # That makes `$!` equal the PGID, so cmd_stop can take down the whole group.
  set -m

  echo "start: launching vite (logs: ${vite_log})" >&2
  (cd "${REPO_ROOT}/daemon/frontend" && exec pnpm dev) \
    >"${vite_log}" 2>&1 &
  local vite_pid=$!
  printf '%s\n' "${vite_pid}" > "${PID_FILE}"

  echo "start: launching daemon (logs: ${daemon_log})" >&2
  (cd "${REPO_ROOT}" && exec env OZMUX_EXTENSION_ROOT="${REPO_ROOT}/extensions" \
    cargo run -p daemon_bootstrap) \
    >"${daemon_log}" 2>&1 &
  local daemon_pid=$!
  printf '%s\n' "${daemon_pid}" >> "${PID_FILE}"

  set +m

  echo "start: waiting for /health (max ${READY_TIMEOUT_SECONDS}s)" >&2
  local deadline=$(( $(date +%s) + READY_TIMEOUT_SECONDS ))
  while (( $(date +%s) < deadline )); do
    if curl -fsS "http://localhost:${VITE_PORT}/health" >/dev/null 2>&1; then
      echo "ready"
      return 0
    fi
    if ! pid_alive "${vite_pid}" || ! pid_alive "${daemon_pid}"; then
      echo "error: a child process exited before readiness." >&2
      echo "---- vite (last 20 lines) ----" >&2; tail -n 20 "${vite_log}" >&2 || true
      echo "---- daemon (last 20 lines) --" >&2; tail -n 20 "${daemon_log}" >&2 || true
      cmd_stop || true
      exit 1
    fi
    sleep 0.5
  done

  echo "error: readiness timeout after ${READY_TIMEOUT_SECONDS}s." >&2
  echo "---- vite (last 20 lines) ----" >&2; tail -n 20 "${vite_log}" >&2 || true
  echo "---- daemon (last 20 lines) --" >&2; tail -n 20 "${daemon_log}" >&2 || true
  cmd_stop || true
  exit 1
}
cmd_stop() {
  if [[ ! -f "${PID_FILE}" ]]; then
    echo "stop: nothing to do (no ${PID_FILE})" >&2
    return 0
  fi

  local -a pids=()
  while IFS= read -r pid; do
    [[ -n "${pid}" ]] && pids+=("${pid}")
  done < "${PID_FILE}"

  # Each pid is a process-group leader (cmd_start ran with `set -m`).
  # `kill -TERM -- -<pid>` signals the whole group, including grandchildren that
  # the spawned process forked (e.g. pnpm's vite child).
  for pid in "${pids[@]}"; do
    if pid_alive "${pid}"; then
      echo "stop: SIGTERM group ${pid}" >&2
      kill -TERM -- "-${pid}" 2>/dev/null || true
    fi
  done

  # Give them up to 5 seconds to exit cleanly.
  local deadline=$(( $(date +%s) + 5 ))
  while (( $(date +%s) < deadline )); do
    local any_alive=0
    for pid in "${pids[@]}"; do
      if pid_alive "${pid}"; then any_alive=1; fi
    done
    [[ "${any_alive}" -eq 0 ]] && break
    sleep 0.2
  done

  for pid in "${pids[@]}"; do
    if pid_alive "${pid}"; then
      echo "stop: SIGKILL group ${pid}" >&2
      kill -KILL -- "-${pid}" 2>/dev/null || true
    fi
  done

  rm -f "${PID_FILE}"
}

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
    echo "       run: make dev-e2e-stop" >&2
    exit 1
  fi
  echo "info: removing stale ${PID_FILE}" >&2
  rm -f "${PID_FILE}"
}

main "$@"
