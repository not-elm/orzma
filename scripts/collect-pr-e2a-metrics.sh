#!/usr/bin/env bash
# PR-E2a metric collection — drive a daemon, run NeoVim jk-scroll
# yourself, then Ctrl+C this script to capture the /metrics snapshot.
#
# Output goes to /tmp/pr-e2a-data/<timestamp>/ with:
#   - metrics-start.prom    (right after daemon comes up; baseline)
#   - metrics-end.prom      (right before daemon shutdown; final)
#   - trace.json            (OZMUX_PERF_TRACE chrome trace)
#   - daemon.log            (tracing output)
#
# Usage:
#   ./scripts/collect-pr-e2a-metrics.sh
#   ... then in a separate terminal connect to the daemon UI and run
#       NeoVim, jk-scroll for 30+ seconds ...
#   ... then Ctrl+C here. The script snapshots /metrics + saves trace.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUTDIR="/tmp/pr-e2a-data/$STAMP"
mkdir -p "$OUTDIR"

cd "$ROOT"

echo "== Build with tracing-chrome feature =="
cargo build --features ozmux_cli/tracing-chrome -p ozmux_cli 2>&1 | tail -3

export OZMUX_METRICS=1
export OZMUX_PERF_TRACE="$OUTDIR/trace.json"
export OZMUX_EXTENSION_ROOT="$ROOT/extensions"

echo ""
echo "== Starting daemon =="
target/debug/ozmux daemon start --foreground > "$OUTDIR/daemon.log" 2>&1 &
DAEMON_PID=$!
echo "DAEMON_PID=$DAEMON_PID"

# Capture /metrics snapshot when SIGINT arrives.
cleanup() {
    echo ""
    echo "== Capturing final /metrics =="
    curl -s http://127.0.0.1:3200/metrics > "$OUTDIR/metrics-end.prom" || true
    echo "== Stopping daemon =="
    kill "$DAEMON_PID" 2>/dev/null || true
    wait "$DAEMON_PID" 2>/dev/null || true
    echo ""
    echo "Data saved to: $OUTDIR"
    echo ""
    echo "Key files:"
    echo "  metrics-end.prom   — final /metrics dump"
    echo "  trace.json         — Chrome trace (open at https://ui.perfetto.dev/)"
    echo "  daemon.log         — tracing log"
    echo ""
    echo "PR-E2a metric values (post-jk-scroll):"
    if [[ -s "$OUTDIR/metrics-end.prom" ]]; then
        grep -E '^ozmux_terminal_(emit_duration_seconds_count|coalesce_wait_seconds_count|snapshot_total|pty_chunk_drops_total)' "$OUTDIR/metrics-end.prom" || echo "  (no ozmux_terminal_* metric samples yet)"
    fi
    exit 0
}
trap cleanup INT TERM

# Wait for /health.
for i in {1..60}; do
    if curl -fs http://127.0.0.1:3200/health >/dev/null 2>&1; then
        echo "Daemon up after $i probes."
        break
    fi
    sleep 0.5
done

sleep 1
curl -s http://127.0.0.1:3200/metrics > "$OUTDIR/metrics-start.prom" || true

echo ""
echo "============================================================"
echo "DAEMON RUNNING. http://127.0.0.1:3200/ to attach a UI."
echo ""
echo "Now in another terminal (or via the daemon UI):"
echo "  1. Open a terminal session"
echo "  2. Run nvim on a real file"
echo "  3. Hold j key for ~30 seconds, then hold k for ~30 seconds"
echo "  4. Exit nvim"
echo "  5. Return HERE and press Ctrl+C to capture /metrics."
echo "============================================================"
echo ""

# Idle wait — cleanup() fires on Ctrl+C / SIGTERM.
while true; do
    sleep 1
done
