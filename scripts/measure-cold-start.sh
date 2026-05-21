#!/usr/bin/env bash
# scripts/measure-cold-start.sh
# Measure cef::initialize cold start time on ozmux-daemon.
# Run from the workspace root.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DAEMON_BIN="$REPO_ROOT/target/debug/ozmux-daemon.app/Contents/MacOS/ozmux-daemon"
RESULTS_DIR="$REPO_ROOT/docs/superpowers/notes/measurements-$(date +%Y-%m-%d)"
OUT="$RESULTS_DIR/cold-start.txt"

mkdir -p "$RESULTS_DIR"

if [ ! -x "$DAEMON_BIN" ]; then
  echo "ozmux-daemon binary not found at $DAEMON_BIN" >&2
  echo "Run: make bundle-ozmux-daemon" >&2
  exit 1
fi

> "$OUT"

for i in $(seq 1 5); do
  # OS disk cache purge (best-effort)
  sudo -n purge 2>/dev/null || echo "(purge skipped; sudo needed for accuracy)"
  make -C "$REPO_ROOT" kill-daemon 2>/dev/null || true
  sleep 3

  # Time from process start until /health responds
  start_ts=$(date +%s.%N)

  RUST_LOG=info "$DAEMON_BIN" > "$RESULTS_DIR/cold-start-${i}.log" 2>&1 &
  dpid=$!

  for _ in $(seq 1 60); do
    if curl -sf http://127.0.0.1:3200/health > /dev/null; then break; fi
    sleep 0.2
  done

  end_ts=$(date +%s.%N)
  elapsed=$(echo "$end_ts - $start_ts" | bc)
  echo "run $i: elapsed = ${elapsed}s" | tee -a "$OUT"

  kill -INT "$dpid" 2>/dev/null || true
  wait "$dpid" 2>/dev/null || true
done

# Stats
awk '/elapsed/ {s+=$NF+0; n++; if (NR==1 || $NF+0 < min) min=$NF+0; if ($NF+0 > max) max=$NF+0} END {if (n>0) printf "\nsamples=%d avg=%.2fs min=%.2fs max=%.2fs\n", n, s/n, min, max}' "$OUT" | tee -a "$OUT"
