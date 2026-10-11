#!/usr/bin/env bash
# Run the RelayHop end-to-end matrix over simulated carrier networks.
#
#   sim/run.sh                     # matriz completa (E1..E8)
#   sim/run.sh --only E4 --secs 15 # um cenário
#   sim/run.sh --list              # lista os cenários
#
# What it does:
#   1. builds the Rust CLI (release) and the Go binaries (worker/probe/derper),
#   2. builds the simulation image (sim/Dockerfile),
#   3. runs the matrix inside one container with NET_ADMIN/SYS_ADMIN/NET_RAW,
#      mounting .sim-bin at /opt/bin and sim/results at /tmp/simout.
#
# The host machine's network, routes and DNS are never touched: everything
# happens in network namespaces created inside the container.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="${RELAYHOP_SIM_BIN:-$ROOT/.sim-bin}"
IMAGE="${RELAYHOP_SIM_IMAGE:-relayhop-sim}"
RESULTS="$ROOT/sim/results"

echo "[sim] compilando o CLI (release)"
cargo build --release --locked --manifest-path "$ROOT/Cargo.toml"
mkdir -p "$BIN_DIR"
cp "$ROOT/target/release/relayhop" "$BIN_DIR/relayhop"

echo "[sim] compilando os binários Go"
"$ROOT/scripts/build-workers.sh" "$BIN_DIR" >/dev/null

echo "[sim] construindo a imagem $IMAGE"
docker build -q -f "$ROOT/sim/Dockerfile" -t "$IMAGE" "$ROOT" >/dev/null

mkdir -p "$RESULTS"
echo "[sim] rodando a matriz (logs em sim/results/)"
exec docker run --rm \
  --cap-add NET_ADMIN --cap-add SYS_ADMIN --cap-add NET_RAW \
  -v "$BIN_DIR:/opt/bin:ro" \
  -v "$RESULTS:/tmp/simout" \
  "$IMAGE" "$@"
