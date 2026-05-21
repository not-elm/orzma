#!/usr/bin/env bash
# scripts/measure-rss.sh
# Measure ozmux-daemon RSS at idle and under 1/2/4 Browser Activities load.
# Run from the workspace root.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DAEMON_BIN="$REPO_ROOT/target/debug/ozmux-daemon.app/Contents/MacOS/ozmux-daemon"
HEALTH_URL="http://127.0.0.1:3200/health"
RESULTS_DIR="$REPO_ROOT/docs/superpowers/notes/measurements-$(date +%Y-%m-%d)"

mkdir -p "$RESULTS_DIR"

if [ ! -x "$DAEMON_BIN" ]; then
  echo "ozmux-daemon binary not found at $DAEMON_BIN" >&2
  echo "Run: make bundle-ozmux-daemon" >&2
  exit 1
fi

# Helper: get RSS in MB
rss_mb_of_pid() {
  local pid="$1"
  ps -o rss= -p "$pid" | awk '{printf "%.1f\n", $1/1024}'
}

# Helper: create N browser activities
create_n_browsers() {
  local n="$1"
  for i in $(seq 1 "$n"); do
    # Minimal: ozmux session new + ozmux browser https://example.com
    # Implementation depends on environment (this plan provides a template only)
    "$REPO_ROOT/target/debug/ozmux" browser "https://example.com" || true
    sleep 2
  done
}

measure_at_load() {
  local label="$1"
  local n_browsers="$2"
  local outfile="$RESULTS_DIR/rss-${label}.txt"

  # Clean state
  make -C "$REPO_ROOT" kill-daemon 2>/dev/null || true
  sleep 2

  # Start daemon
  "$DAEMON_BIN" > "$RESULTS_DIR/daemon-${label}.log" 2>&1 &
  local dpid=$!
  echo "started ozmux-daemon (pid $dpid) for label '$label'" | tee -a "$outfile"

  # Wait for /health
  for _ in $(seq 1 30); do
    if curl -sf "$HEALTH_URL" > /dev/null; then break; fi
    sleep 1
  done

  # Idle RSS (right after startup)
  sleep 5
  local idle_rss
  idle_rss="$(rss_mb_of_pid "$dpid")"
  echo "idle_rss_mb = $idle_rss" | tee -a "$outfile"

  # Create N browser activities
  if [ "$n_browsers" -gt 0 ]; then
    create_n_browsers "$n_browsers"
    sleep 10  # let frames flow
    local loaded_rss
    loaded_rss="$(rss_mb_of_pid "$dpid")"
    echo "loaded_rss_mb (${n_browsers}_browsers) = $loaded_rss" | tee -a "$outfile"

    # Helper processes
    local helper_rss_total
    helper_rss_total="$(pgrep -f 'ozmux-daemon Helper' | xargs -I{} ps -o rss= -p {} 2>/dev/null | awk '{sum+=$1} END {printf "%.1f\n", sum/1024}')"
    echo "helper_rss_total_mb = $helper_rss_total" | tee -a "$outfile"
    echo "combined_rss_mb = $(echo "$loaded_rss + $helper_rss_total" | bc)" | tee -a "$outfile"
  fi

  # Soak: hold for 60 s and measure max RSS
  local max_rss=0
  for _ in $(seq 1 12); do
    sleep 5
    local cur
    cur="$(rss_mb_of_pid "$dpid" 2>/dev/null || echo 0)"
    if (( $(echo "$cur > $max_rss" | bc -l) )); then
      max_rss="$cur"
    fi
  done
  echo "soak_60s_max_rss_mb = $max_rss" | tee -a "$outfile"

  # Tear down
  kill -INT "$dpid" 2>/dev/null || true
  wait "$dpid" 2>/dev/null || true
}

# Run scenarios
measure_at_load "idle" 0
measure_at_load "1browser" 1
measure_at_load "2browsers" 2
measure_at_load "4browsers" 4

echo
echo "Results saved to: $RESULTS_DIR/"
