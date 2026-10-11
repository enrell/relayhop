#!/usr/bin/env bash
# Build every Go binary RelayHop needs, into one directory:
#
#   relayhop-tailcat  supervised worker (worker/tailcat/relay)
#   probe             Tailcat measurement instrument (worker/tailcat/probe)
#   derper            private DERP server for simulations (worker/tailcat/sim/derper)
#
# Usage: scripts/build-workers.sh [outdir]   (default: .sim-bin)
#
# Versions are pinned in the three go.mod files; nothing is fetched at runtime.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/.sim-bin}"
mkdir -p "$OUT"

echo "[build] worker  -> $OUT/relayhop-tailcat"
(cd "$ROOT/worker/tailcat/relay" && go build -o "$OUT/relayhop-tailcat" .)

echo "[build] probe   -> $OUT/probe"
(cd "$ROOT/worker/tailcat/probe" && go build -o "$OUT/probe" .)

echo "[build] derper  -> $OUT/derper"
(cd "$ROOT/worker/tailcat/sim/derper" && go build -o "$OUT/derper" tailscale.com/cmd/derper)

ls -l "$OUT"
