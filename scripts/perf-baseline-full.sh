#!/usr/bin/env bash
set -euo pipefail

# Generate a perf-baseline bundle: trace.json, perf-report.json, metrics.txt,
# bench.json, system.json. Requires the tracing-chrome feature in the daemon
# binary; the script fails fast if OZMUX_PERF_TRACE is set without it.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${OUTPUT_DIR:-$ROOT/baseline-$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$OUTPUT_DIR"

cd "$ROOT"

# Build the CLI with tracing-chrome so OZMUX_PERF_TRACE produces a file.
cargo build --features ozmux_cli/tracing-chrome -p ozmux_cli

export OZMUX_PERF_PRODUCED_AT=1
export OZMUX_PERF_TRACE="$OUTPUT_DIR/trace.json"
export OZMUX_METRICS=1
export OZMUX_EXTENSION_ROOT="$ROOT/extensions"

# Start daemon in background.
target/debug/ozmux daemon start --foreground &
DAEMON_PID=$!
trap 'kill $DAEMON_PID $VITE_PID 2>/dev/null || true' EXIT
until curl -fs http://127.0.0.1:3200/health >/dev/null 2>&1; do
    sleep 0.1
done

# Start Vite dev server for Playwright.
pnpm --filter ozmux-ui exec vite --port 5173 &
VITE_PID=$!
until curl -fs http://localhost:5173/ >/dev/null 2>&1; do
    sleep 0.1
done

# Drive the perf-baseline Playwright spec.
OZMUX_PERF_REPORT_OUT="$OUTPUT_DIR/perf-report.json" \
    pnpm --filter ozmux-ui exec playwright test perf-baseline

# Snapshot /metrics.
curl -s http://127.0.0.1:3200/metrics > "$OUTPUT_DIR/metrics.txt"

# Stop daemon and Vite (trap will also kill, but explicit is clearer).
kill "$DAEMON_PID" 2>/dev/null || true
kill "$VITE_PID" 2>/dev/null || true
trap - EXIT

# Run criterion bench (--quick mode for reasonable wall time).
cargo bench -p ozmux_terminal --bench broadcast_lag_rate -- --quick 2>&1 | \
    tee "$OUTPUT_DIR/bench.txt"

# System info.
{
    echo "{"
    echo "  \"rustc\": \"$(rustc --version)\","
    echo "  \"uname\": \"$(uname -a)\","
    echo "  \"date\": \"$(date -Iseconds)\""
    echo "}"
} > "$OUTPUT_DIR/system.json"

echo "perf-baseline bundle: $OUTPUT_DIR"
