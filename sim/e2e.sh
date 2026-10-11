#!/usr/bin/env bash
# RelayHop end-to-end matrix: the REAL application (host + watch) over
# simulated carrier networks. Runs inside the sim container (see Dockerfile);
# `sim/run.sh` is the host-side entry point.
#
# Each scenario:
#   1. shapes the topology/impairment,
#   2. starts `relayhop host` inside ns pa and `relayhop watch <convite>` inside
#      ns pb (separate RELAYHOP_DATA_DIR so the instance lock is per side),
#   3. asserts on the JSON reports (frames, bytes, bitrate, drops, path),
#   4. appends one JSON line to /tmp/simout/results.jsonl.
#
# Usage: e2e.sh [--only NAME] [--secs N] [--list] [--keep-logs]
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=topo.sh
source "$HERE/topo.sh"

SECS="${SIM_SECS:-20}"
ONLY=""
LIST_ONLY=0
OUT=/tmp/simout
FAILURES=0
PASSED=0

while [ $# -gt 0 ]; do
  case "$1" in
    --only) ONLY="$2"; shift 2 ;;
    --secs) SECS="$2"; shift 2 ;;
    --list) LIST_ONLY=1; shift ;;
    *) die "argumento desconhecido: $1" ;;
  esac
done

mkdir -p "$OUT"
: > "$OUT/results.jsonl"

json_str() { python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'; }

# ---------------------------------------------------------------- session ---
# run_session NAME HOST_ARGS... -- WATCH_ARGS...
run_session() {
  local name="$1"; shift
  local host_extra=() watch_extra=()
  local seen=0
  for arg in "$@"; do
    if [ "$arg" = "--" ]; then seen=1; continue; fi
    if [ "$seen" = 0 ]; then host_extra+=("$arg"); else watch_extra+=("$arg"); fi
  done

  local pa_state="/tmp/state/$name-pa" pb_state="/tmp/state/$name-pb"
  rm -rf "$pa_state" "$pb_state"; mkdir -p "$pa_state" "$pb_state"
  local hout="$OUT/$name-host.out" herr="$OUT/$name-host.err"
  local wout="$OUT/$name-watch.out" werr="$OUT/$name-watch.err"
  : > "$hout"; : > "$herr"; : > "$wout"; : > "$werr"

  derp_counters_install
  local derp_before; derp_before="$(derp_counters_read)"
  local start_epoch; start_epoch=$(date +%s)

  ip netns exec pa env \
    RELAYHOP_TAILCAT_BIN="$BIN/relayhop-tailcat" \
    RELAYHOP_DATA_DIR="$pa_state" \
    SSL_CERT_FILE="$CERTS/ca.crt" \
    "$BIN/relayhop" host --json --secs "$((SECS + 30))" --derp-map "$MAPURL" \
    "${host_extra[@]}" > "$hout" 2> "$herr" &
  local host_pid=$!

  local token="" waited=0
  while [ $waited -lt 120 ]; do
    token="$(grep -m1 '^rh1\.' "$hout" 2>/dev/null || true)"
    [ -n "$token" ] && break
    kill -0 "$host_pid" 2>/dev/null || break
    sleep 0.5; waited=$((waited + 1))
  done

  if [ -z "$token" ]; then
    sleep 1
    kill "$host_pid" 2>/dev/null || true; wait "$host_pid" 2>/dev/null || true
    derp_counters_remove
    # No invite: the session never opened. Whether that is a pass or a fail
    # depends on the scenario (E7 expects the relay to be unreachable).
    SIM_NAME="$name" \
    SIM_HOST_OUT="$hout" SIM_HOST_ERR="$herr" \
    SIM_WATCH_OUT="$wout" SIM_WATCH_ERR="$werr" \
    SIM_SECS="$SECS" \
    SIM_WALL=$(( $(date +%s) - start_epoch )) \
    SIM_DERP_BEFORE="$derp_before" SIM_DERP_AFTER="$derp_before" \
    python3 "$HERE/assert.py"
    return $?
  fi

  local wstart; wstart=$(date +%s)
  ip netns exec pb env \
    RELAYHOP_TAILCAT_BIN="$BIN/relayhop-tailcat" \
    RELAYHOP_DATA_DIR="$pb_state" \
    SSL_CERT_FILE="$CERTS/ca.crt" \
    "$BIN/relayhop" watch "$token" --json --secs "$SECS" --derp-map "$MAPURL" \
    "${watch_extra[@]}" > "$wout" 2> "$werr" &
  local watch_pid=$!

  local wwaited=0
  while [ $wwaited -lt $((SECS + 90)) ]; do
    kill -0 "$watch_pid" 2>/dev/null || break
    sleep 1; wwaited=$((wwaited + 1))
  done
  kill "$watch_pid" 2>/dev/null || true; wait "$watch_pid" 2>/dev/null || true
  # Ask the host to stop the way a user would (Ctrl-C): it must finish the
  # session, close the worker and print its report. SIGTERM would hide that.
  if kill -0 "$host_pid" 2>/dev/null; then
    kill -INT "$host_pid" 2>/dev/null || true
    local hwaited=0
    while [ $hwaited -lt 20 ]; do
      kill -0 "$host_pid" 2>/dev/null || break
      sleep 0.5; hwaited=$((hwaited + 1))
    done
  fi
  kill "$host_pid" 2>/dev/null || true; wait "$host_pid" 2>/dev/null || true

  local derp_after; derp_after="$(derp_counters_read)"
  derp_counters_remove

  SIM_NAME="$name" \
  SIM_HOST_OUT="$hout" SIM_HOST_ERR="$herr" \
  SIM_WATCH_OUT="$wout" SIM_WATCH_ERR="$werr" \
  SIM_SECS="$SECS" \
  SIM_WALL=$(( $(date +%s) - wstart )) \
  SIM_DERP_BEFORE="$derp_before" SIM_DERP_AFTER="$derp_after" \
  python3 "$HERE/assert.py"
  return $?
}

# ---------------------------------------------------------------- matrix ----
scenario_E1() { # LAN direta: mesmo segmento L2, sem NAT no caminho
  lan_link_up
  run_session E1-lan
  local rc=$?
  lan_link_down
  return $rc
}

scenario_E2() { # NAT cone: hairpin duplo-CGNAT (relay esperado, mídia obrigatória)
  run_session E2-cone-nat
}

scenario_E3() { # NAT simétrico: porta mapeada imprevisível
  symmetric_nat
  run_session E3-symmetric-nat
  cone_nat
}

scenario_E4() { # UDP bloqueado entre os pares: DERP (TCP) precisa carregar a mídia
  udp_block_on
  run_session E4-udp-blocked
  local rc=$?
  udp_block_off
  return $rc
}

scenario_E5() { # 60 ms + 2% de perda por sentido: degradado, mas vivo
  netem_on
  run_session E5-lossy
  local rc=$?
  netem_off
  return $rc
}

scenario_E9() { # 20 ms + 1% de perda: perda leve, entrega esperada
  netem_on 20ms 1%
  run_session E9-mild-loss
  local rc=$?
  netem_off
  return $rc
}

scenario_E6() { # IPv6 global (sem NAT) no mesmo enlace
  v6_link_up
  run_session E6-ipv6
  local rc=$?
  v6_link_down
  return $rc
}

scenario_E7() { # relay fora do ar: erro acionável, sem travar
  derp_block_on
  run_session E7-derp-down
  local rc=$?
  derp_block_off
  return $rc
}

scenario_E8() { # troca de rede no meio da sessão: caminho direto cai
  lan_link_up
  (
    sleep 8
    log "E8: derrubando o enlace direto (troca de rede do transmissor)"
    derp_counters_read > /tmp/simout/E8-derp-at-drop.txt 2>/dev/null || true
    lan_link_down
  ) &
  local drop_pid=$!
  run_session E8-network-change
  local rc=$?
  # Wait only for the helper: `wait` with no argument would also wait for the
  # derper and the derpmap server started in the background.
  wait "$drop_pid" 2>/dev/null || true
  lan_link_down
  return $rc
}

ALL=(E1 E2 E3 E4 E5 E9 E6 E7 E8)

if [ "$LIST_ONLY" = 1 ]; then
  printf '%s\n' "${ALL[@]}"
  exit 0
fi

need_binaries
make_certs
cleanup_all
trap cleanup_all EXIT
ns_up
derper_start
log "infra pronta; cenas: ${ALL[*]}"

for name in "${ALL[@]}"; do
  if [ -n "$ONLY" ] && [ "$name" != "$ONLY" ]; then continue; fi
  log "=== $name ==="
  if "scenario_$name"; then
    PASSED=$((PASSED + 1))
  else
    FAILURES=$((FAILURES + 1))
    log "$name FALHOU (ver $OUT/$name-*.err)"
  fi
done

echo "SUMMARY {\"passed\":$PASSED,\"failed\":$FAILURES,\"secs_per_session\":$SECS,\"kernel\":\"$(uname -r)\"}"
[ "$FAILURES" = 0 ]
