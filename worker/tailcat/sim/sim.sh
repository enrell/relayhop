#!/bin/bash
# CGNAT simulation matrix for the Tailcat gate (PLAN.md phase 1).
#
# Everything runs inside ONE container (needs NET_ADMIN + SYS_ADMIN for
# netns/iptables/tc); the host is untouched. Topology models two carrier
# NATs peered over an internet core — each NAT is its OWN namespace, because
# hairpin between two public IPs local to the same netns can never enter
# the forward path (and real CGNATs are separate boxes):
#
#   ns pa (100.64.1.10) --[sub-a]--> ns nat-a --[pub fabric 203.0.113.0/24]--+
#                                                                            +-- brpub (bridge in ns core)
#   ns pb (100.64.2.10) --[sub-b]--> ns nat-b --[pub fabric ---------------]+
#   ns core also serves derper :443 + derpmap :8080 on 203.0.113.10.
#
# The public IPs (.21/.22) live on the NAT boxes' external interfaces, so
# inbound hairpin traverses PREROUTING conntrack exactly like a carrier box.
# The kernel's strict reverse-path filter passes everywhere because every
# source arrives on an interface whose connected route covers it.
set -euo pipefail

BIN="${SIM_BIN:-/opt/bin}"
CERTS=/tmp/sim-certs
WWW=/tmp/sim-www
ADDR=/tmp/sim-tc.addr
MAPURL="http://203.0.113.10:8080/derpmap.json"
PUBA=203.0.113.21
PUBB=203.0.113.22
DERP=203.0.113.10
export SSL_CERT_FILE="$CERTS/ca.crt"

log() { echo "[sim] $*" >&2; }
die() { echo "[sim] FATAL: $*" >&2; exit 1; }

[ -x "$BIN/probe" ] || die "probe ausente em $BIN (monte -v <dir>:/opt/bin:ro)"
[ -x "$BIN/derper" ] || die "derper ausente em $BIN (monte -v <dir>:/opt/bin:ro)"

cleanup() {
  for n in pa pb nat-a nat-b core; do ip netns del "$n" 2>/dev/null || true; done
}
trap cleanup EXIT
cleanup

# --- certs (fresh CA + server cert every run) ---
mkdir -p "$CERTS" "$WWW"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=sim-ca" \
  -keyout "$CERTS/ca.key" -out "$CERTS/ca.crt" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=derp.test" \
  -keyout "$CERTS/derp.test.key" -out "$CERTS/derp.test.csr" 2>/dev/null
cat > "$CERTS/ext.cnf" <<'EOF'
subjectAltName = DNS:derp.test, IP:203.0.113.10
EOF
openssl x509 -req -days 2 -in "$CERTS/derp.test.csr" -CA "$CERTS/ca.crt" \
  -CAkey "$CERTS/ca.key" -CAcreateserial -extfile "$CERTS/ext.cnf" \
  -out "$CERTS/derp.test.crt" 2>/dev/null
cp /etc/sim/derpmap.json "$WWW/derpmap.json"

# --- namespaces ---
for n in core nat-a nat-b pa pb; do ip netns add "$n"; done

# --- public fabric: one bridge in ns core, both NAT boxes L2-adjacent ---
ip link add c-puba type veth peer name nat-puba
ip link add c-pubb type veth peer name nat-pubb
ip link set nat-puba netns nat-a
ip link set nat-pubb netns nat-b
ip link set c-puba netns core
ip link set c-pubb netns core
ip netns exec core bash -c '
  ip link set lo up
  ip link add brpub type bridge
  ip link set c-puba master brpub
  ip link set c-pubb master brpub
  ip addr add 203.0.113.10/24 dev brpub
  ip link set c-puba up; ip link set c-pubb up; ip link set brpub up'
ip netns exec nat-a bash -c 'ip link set lo up; ip addr add 203.0.113.21/24 dev nat-puba; ip link set nat-puba up'
ip netns exec nat-b bash -c 'ip link set lo up; ip addr add 203.0.113.22/24 dev nat-pubb; ip link set nat-pubb up'

# --- subscriber links (real CGNAT range) ---
ip link add a-sub type veth peer name pa-eth
ip link add b-sub type veth peer name pb-eth
ip link set pa-eth netns pa
ip link set pb-eth netns pb
ip link set a-sub netns nat-a
ip link set b-sub netns nat-b
ip netns exec nat-a bash -c 'ip addr add 100.64.1.1/24 dev a-sub; ip link set a-sub up'
ip netns exec nat-b bash -c 'ip addr add 100.64.2.1/24 dev b-sub; ip link set b-sub up'
ip netns exec pa bash -c 'ip link set lo up; ip addr add 100.64.1.10/24 dev pa-eth; ip link set pa-eth up; ip route add default via 100.64.1.1'
ip netns exec pb bash -c 'ip link set lo up; ip addr add 100.64.2.10/24 dev pb-eth; ip link set pb-eth up; ip route add default via 100.64.2.1'

# --- NAT boxes: forward + cone SNAT ---
for box in nat-a nat-b; do
  ip netns exec "$box" iptables -P FORWARD ACCEPT
done
ip netns exec nat-a iptables -t nat -A POSTROUTING -s 100.64.1.0/24 -j SNAT --to-source "$PUBA"
ip netns exec nat-b iptables -t nat -A POSTROUTING -s 100.64.2.0/24 -j SNAT --to-source "$PUBB"

# --- derper + derpmap inside ns core ---
ip netns exec core "$BIN/derper" --hostname=derp.test --certmode=manual --certdir="$CERTS" \
  -a "$DERP:443" --http-port -1 > /tmp/derper.log 2>&1 &
DERPER_PID=$!
ip netns exec core python3 -u -m http.server 8080 --bind "$DERP" --directory "$WWW" \
  > /tmp/mapserver.log 2>&1 &
for i in $(seq 1 50); do
  if ip netns exec core timeout 2 bash -c "(echo > /dev/tcp/$DERP/443)" 2>/dev/null; then break; fi
  sleep 0.2
done
(ip netns exec core timeout 2 bash -c "(echo > /dev/tcp/$DERP/443)" 2>/dev/null) || { tail -20 /tmp/derper.log; die "derper não subiu"; }
for i in $(seq 1 50); do
  if ip netns exec core timeout 2 bash -c "(echo > /dev/tcp/$DERP/8080)" 2>/dev/null; then break; fi
  sleep 0.2
done
# Fetch via pa (through NAT-A): proves the subscriber path, not just core-local.
ip netns exec pa python3 -c 'import sys,urllib.request; urllib.request.urlopen(sys.argv[1], timeout=5).read()' "$MAPURL" || die "derpmap inacessível via pa (NAT-A)"
log "infra pronta (derper pid $DERPER_PID)"

run_case() { # name -- extra probe-run args...
  local name="$1"; shift
  if [ -n "${ONLY:-}" ] && [ "$name" != "$ONLY" ]; then
    log "$name pulado (ONLY=$ONLY)"
    return
  fi
  rm -f "$ADDR"
  ip netns exec pa timeout 150 "$BIN/probe" serve --udp-port 4107 --verbose \
    --derpmap-url "$MAPURL" --addr-file "$ADDR" > /tmp/serve.log 2>&1 &
  local srv=$!
  for i in $(seq 1 100); do [ -s "$ADDR" ] && break; sleep 0.2; done
  [ -s "$ADDR" ] || { kill "$srv" 2>/dev/null; tail -20 /tmp/serve.log; die "$name: servidor não publicou endereço"; }
  # DERP TCP byte counters on both NAT boxes: relay traffic shows up here,
  # direct P2P traffic does not.
  for box in nat-a nat-b; do
    ip netns exec "$box" iptables -I FORWARD -p tcp -s "$DERP" --sport 443 -j ACCEPT
    ip netns exec "$box" iptables -I FORWARD -p tcp -d "$DERP" --dport 443 -j ACCEPT
  done
  local out
  if out=$(ip netns exec pb timeout 150 "$BIN/probe" run --addr "$(cat "$ADDR")" \
      --derpmap-url "$MAPURL" --udp-port 4107 ${PROBE_ARGS:-} "$@" 2>/tmp/client.err); then
    echo "RESULT $name $out"
  else
    echo "RESULT $name {\"ok\":false,\"error\":\"client exit nonzero\",\"stderr\":$(head -c 300 /tmp/client.err | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')}"
    echo "DEBUG $name derper_tail=$(tail -c 600 /tmp/derper.log 2>/dev/null | tr '\n' '|')"
    echo "DEBUG $name serve_tail=$(tail -c 800 /tmp/serve.log 2>/dev/null | tr '\n' '|')"
    echo "DEBUG $name client_tail=$(tail -25 /tmp/client.err 2>/dev/null | tr '\n' '|')"
    echo "DEBUG $name disco=$(grep -ah 'endpoints changed\|call-me-maybe\|Adding endpoint\|best path\|switched.*path\|direct' /tmp/serve.log /tmp/client.err 2>/dev/null | tail -8 | tr '\n' '|')"
  fi
  kill "$srv" 2>/dev/null || true
  wait "$srv" 2>/dev/null || true
  echo "NET $name derp_bytes_a=$(ip netns exec nat-a iptables -L FORWARD -n -v -x 2>/dev/null | awk '/dpt:443|spt:443/ {printf "%s:%s ", $1, $2}')"
  echo "NET $name derp_bytes_b=$(ip netns exec nat-b iptables -L FORWARD -n -v -x 2>/dev/null | awk '/dpt:443|spt:443/ {printf "%s:%s ", $1, $2}')"
  echo "NET $name $(grep -ah 'endpoints changed' /tmp/serve.log /tmp/client.err 2>/dev/null | tail -4 | tr '\n' '|')"
  for box in nat-a nat-b; do
    ip netns exec "$box" iptables -D FORWARD -p tcp -s "$DERP" --sport 443 -j ACCEPT 2>/dev/null || true
    ip netns exec "$box" iptables -D FORWARD -p tcp -d "$DERP" --dport 443 -j ACCEPT 2>/dev/null || true
  done
  if [ -d /tmp/simout ]; then
    cp /tmp/serve.log "/tmp/simout/$name-serve.log" 2>/dev/null || true
    cp /tmp/client.err "/tmp/simout/$name-client.log" 2>/dev/null || true
  fi
  if [ -n "${NETDEBUG:-}" ]; then
    echo "DEBUG $name pa_route=$(ip netns exec pa ip route show | tr '\n' ';')"
    echo "DEBUG $name pb_route=$(ip netns exec pb ip route show | tr '\n' ';')"
  fi
}

STD=(--rtt-count 30 --rtt-size 1000 --blast-mbps 5 --blast-secs 3)

log "S1: cone NAT, UDP liberado (baseline direto)"
run_case S1 "${STD[@]}"

log "S1b: cone NAT, datagrama máximo 1232"
run_case S1b --rtt-count 30 --rtt-size 1232 --blast-mbps 1 --blast-secs 1

log "S2: UDP direto bloqueado nos NATs (fallback relay)"
ip netns exec nat-a iptables -A FORWARD -p udp -s 100.64.1.0/24 -d "$PUBB" -j DROP
ip netns exec nat-b iptables -A FORWARD -p udp -s 100.64.2.0/24 -d "$PUBA" -j DROP
run_case S2 "${STD[@]}"
ip netns exec nat-a iptables -D FORWARD -p udp -s 100.64.1.0/24 -d "$PUBB" -j DROP
ip netns exec nat-b iptables -D FORWARD -p udp -s 100.64.2.0/24 -d "$PUBA" -j DROP

log "S3: cone NAT + 60ms/2% loss (netem)"
ip netns exec nat-a tc qdisc add dev a-sub root netem delay 60ms loss 2%
ip netns exec nat-b tc qdisc add dev b-sub root netem delay 60ms loss 2%
run_case S3 --rtt-count 30 --rtt-size 1000 --blast-mbps 3 --blast-secs 3
ip netns exec nat-a tc qdisc del dev a-sub root
ip netns exec nat-b tc qdisc del dev b-sub root

log "S4: NAT simétrico (porta mapeada imprevisível por destino)"
ip netns exec nat-a iptables -t nat -D POSTROUTING -s 100.64.1.0/24 -j SNAT --to-source "$PUBA"
ip netns exec nat-b iptables -t nat -D POSTROUTING -s 100.64.2.0/24 -j SNAT --to-source "$PUBB"
ip netns exec nat-a iptables -t nat -A POSTROUTING -s 100.64.1.0/24 -j SNAT --to-source "$PUBA" --random
ip netns exec nat-b iptables -t nat -A POSTROUTING -s 100.64.2.0/24 -j SNAT --to-source "$PUBB" --random
run_case S4 "${STD[@]}"
ip netns exec nat-a iptables -t nat -D POSTROUTING -s 100.64.1.0/24 -j SNAT --to-source "$PUBA" --random
ip netns exec nat-b iptables -t nat -D POSTROUTING -s 100.64.2.0/24 -j SNAT --to-source "$PUBB" --random
ip netns exec nat-a iptables -t nat -A POSTROUTING -s 100.64.1.0/24 -j SNAT --to-source "$PUBA"
ip netns exec nat-b iptables -t nat -A POSTROUTING -s 100.64.2.0/24 -j SNAT --to-source "$PUBB"

log "S5: IPv6 global mesmo enlace (2001:db8, sem NAT)"
ip link add v-ab type veth peer name v-ba
ip link set v-ab netns pa
ip link set v-ba netns pb
ip netns exec pa bash -c 'ip addr add 2001:db8:99::10/64 dev v-ab; ip link set v-ab up'
ip netns exec pb bash -c 'ip addr add 2001:db8:99::20/64 dev v-ba; ip link set v-ba up'
run_case S5 "${STD[@]}"

log "S5b: IPv6 + UDP IPv4 bloqueado (fallback entre famílias)"
ip netns exec nat-a iptables -A FORWARD -p udp -s 100.64.1.0/24 -d "$PUBB" -j DROP
ip netns exec nat-b iptables -A FORWARD -p udp -s 100.64.2.0/24 -d "$PUBA" -j DROP
run_case S5b "${STD[@]}"
ip netns exec nat-a iptables -D FORWARD -p udp -s 100.64.1.0/24 -d "$PUBB" -j DROP
ip netns exec nat-b iptables -D FORWARD -p udp -s 100.64.2.0/24 -d "$PUBA" -j DROP
ip netns exec pa ip link del v-ab 2>/dev/null || true

DERPER_RSS=$(ps -o rss= -p "$DERPER_PID" 2>/dev/null | tr -d ' ' || echo unknown)
echo "SUMMARY {\"derper_rss_kb\":\"$DERPER_RSS\",\"kernel\":\"$(uname -r)\"}"
log "matriz concluída"
