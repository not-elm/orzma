#!/usr/bin/env bash
# scripts/soak-test.sh
# Long-running soak test: daemon runs continuously, log crashes.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DAEMON_BIN="$REPO_ROOT/target/debug/ozmux-daemon.app/Contents/MacOS/ozmux-daemon"
LOG_DIR="$REPO_ROOT/docs/superpowers/notes/soak-$(date +%Y-%m-%d)"

mkdir -p "$LOG_DIR"

if [ ! -x "$DAEMON_BIN" ]; then
  echo "ozmux-daemon binary not found at $DAEMON_BIN" >&2
  echo "Run: make bundle-ozmux-daemon" >&2
  exit 1
fi

crash_count=0
restart_count=0

while true; do
  echo "[$(date)] starting ozmux-daemon (run #$((restart_count+1)))"
  start_ts=$(date +%s)
  "$DAEMON_BIN" > "$LOG_DIR/run-$((restart_count+1)).log" 2>&1 &
  pid=$!

  wait "$pid" || true
  exit_code=$?
  end_ts=$(date +%s)
  uptime=$((end_ts - start_ts))

  echo "[$(date)] daemon exited (code $exit_code) after ${uptime}s uptime" \
    | tee -a "$LOG_DIR/soak-summary.txt"

  if [ "$exit_code" -ne 0 ] && [ "$uptime" -gt 60 ]; then
    crash_count=$((crash_count + 1))
    echo "  -> classified as CRASH (total crashes: $crash_count)" | tee -a "$LOG_DIR/soak-summary.txt"
  fi
  restart_count=$((restart_count + 1))

  # Restart after 5 s
  sleep 5
done
