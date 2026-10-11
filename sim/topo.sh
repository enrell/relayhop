#!/usr/bin/env bash
# Network topology helpers for the RelayHop end-to-end harness.
#
# Everything lives inside ONE container (NET_ADMIN + SYS_ADMIN + NET_RAW); the
# host machine is never touched. Topology models two subscribers behind their
# own carrier NAT, an internet core that also runs a private `derper` + derpmap
# server, and optional direct links (LAN L2 / IPv6) used to exercise the direct
# P2P path:
#
#   ns pa (100.64.1.10) --[sub-a]--> ns nat-a --[pub fabric 203.0.113.0/24]--+
#                                                                            +-- brpub (ns core)
#   ns pb (100.64.2.10) --[sub-b]--> ns nat-b --[pub fabric ---------------] +
#   ns core serves derper :443 + derpmap :8080 on 203.0.113.10.
#
# Each NAT is its own namespace: hairpin between two public IPs local to the
# same netns never enters the forward path, and real carrier NATs are separate
# boxes too.
#
# Sourced by e2e.sh; not executable on its own.
set -euo pipefail

BIN="${SIM_BIN:-/opt/bin}"
CERTS=/tmp/sim-certs
WWW=/tmp/sim-www
MAPURL="${SIM_MAPURL:-http://203.0.113.10:8080/derpmap.json}"
PUBA=203.0.113.21
PUBB=203.0.113.22
DERP=203.0.113.10
export SSL_CERT_FILE="$CERTS/ca.crt"

log() { echo "[sim] $*" >&2; }
die() { echo "[sim] FATAL: $*" >&2; exit 1; }

need_binaries() {
  [ -x "$BIN/relayhop" ] || die "relayhop ausente em $BIN (rode scripts/build-workers.sh e monte -v <dir>:$BIN:ro)"
  [ -x "$BIN/relayhop-tailcat" ] || die "relayhop-tailcat ausente em $BIN"
  [ -x "$BIN/derper" ] || die "derper ausente em $BIN"
}

# --- certificates: fresh CA + server cert every run -------------------------
make_certs() {
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
}

# --- namespaces -------------------------------------------------------------
ns_up() {
  for n in core nat-a nat-b pa pb; do ip netns add "$n" 2>/dev/null || true; done

  ip link add c-puba type veth peer name nat-puba 2>/dev/null || true
  ip link add c-pubb type veth peer name nat-pubb 2>/dev/null || true
  ip link set nat-puba netns nat-a
  ip link set nat-pubb netns nat-b
  ip link set c-puba netns core
  ip link set c-pubb netns core
  ip netns exec core bash -c '
    ip link set lo up
    ip link add brpub type bridge 2>/dev/null || true
    ip link set c-puba master brpub
    ip link set c-pubb master brpub
    ip addr add 203.0.113.10/24 dev brpub
    ip link set c-puba up; ip link set c-pubb up; ip link set brpub up'
  ip netns exec nat-a bash -c 'ip link set lo up; ip addr add 203.0.113.21/24 dev nat-puba; ip link set nat-puba up'
  ip netns exec nat-b bash -c 'ip link set lo up; ip addr add 203.0.113.22/24 dev nat-pubb; ip link set nat-pubb up'

  ip link add a-sub type veth peer name pa-eth 2>/dev/null || true
  ip link add b-sub type veth peer name pb-eth 2>/dev/null || true
  ip link set pa-eth netns pa
  ip link set pb-eth netns pb
  ip link set a-sub netns nat-a
  ip link set b-sub netns nat-b
  ip netns exec nat-a bash -c 'ip addr add 100.64.1.1/24 dev a-sub; ip link set a-sub up'
  ip netns exec nat-b bash -c 'ip addr add 100.64.2.1/24 dev b-sub; ip link set b-sub up'
  ip netns exec pa bash -c 'ip link set lo up; ip addr add 100.64.1.10/24 dev pa-eth; ip link set pa-eth up; ip route add default via 100.64.1.1'
  ip netns exec pb bash -c 'ip link set lo up; ip addr add 100.64.2.10/24 dev pb-eth; ip link set pb-eth up; ip route add default via 100.64.2.1'

  for box in nat-a nat-b; do ip netns exec "$box" iptables -P FORWARD ACCEPT; done
  cone_nat
}

cone_nat() {
  ip netns exec nat-a iptables -t nat -D POSTROUTING -s 100.64.1.0/24 -j SNAT --to-source "$PUBA" --random 2>/dev/null || true
  ip netns exec nat-b iptables -t nat -D POSTROUTING -s 100.64.2.0/24 -j SNAT --to-source "$PUBB" --random 2>/dev/null || true
  ip netns exec nat-a iptables -t nat -A POSTROUTING -s 100.64.1.0/24 -j SNAT --to-source "$PUBA"
  ip netns exec nat-b iptables -t nat -A POSTROUTING -s 100.64.2.0/24 -j SNAT --to-source "$PUBB"
}

symmetric_nat() {
  ip netns exec nat-a iptables -t nat -D POSTROUTING -s 100.64.1.0/24 -j SNAT --to-source "$PUBA" 2>/dev/null || true
  ip netns exec nat-b iptables -t nat -D POSTROUTING -s 100.64.2.0/24 -j SNAT --to-source "$PUBB" 2>/dev/null || true
  ip netns exec nat-a iptables -t nat -A POSTROUTING -s 100.64.1.0/24 -j SNAT --to-source "$PUBA" --random
  ip netns exec nat-b iptables -t nat -A POSTROUTING -s 100.64.2.0/24 -j SNAT --to-source "$PUBB" --random
}

# --- optional direct paths --------------------------------------------------
# LAN: pa and pb share one L2 segment, no NAT in the way (10.99.0.0/24).
lan_link_up() {
  ip link add lan-ab type veth peer name lan-ba
  ip link set lan-ab netns pa
  ip link set lan-ba netns pb
  ip netns exec pa bash -c 'ip addr add 10.99.0.10/24 dev lan-ab; ip link set lan-ab up'
  ip netns exec pb bash -c 'ip addr add 10.99.0.20/24 dev lan-ba; ip link set lan-ba up'
}
lan_link_down() {
  ip netns exec pa ip link del lan-ab 2>/dev/null || true
}

# IPv6: global-looking v6 on a dedicated link (2001:db8:99::/64, no NAT).
v6_link_up() {
  ip link add v6-ab type veth peer name v6-ba
  ip link set v6-ab netns pa
  ip link set v6-ba netns pb
  ip netns exec pa bash -c 'ip addr add 2001:db8:99::10/64 dev v6-ab; ip link set v6-ab up'
  ip netns exec pb bash -c 'ip addr add 2001:db8:99::20/64 dev v6-ba; ip link set v6-ba up'
}
v6_link_down() {
  ip netns exec pa ip link del v6-ab 2>/dev/null || true
}

# --- impairments ------------------------------------------------------------
# Drop UDP between the two public addresses: STUN/disco dies, DERP (TCP 443) survives.
udp_block_on() {
  ip netns exec nat-a iptables -A FORWARD -p udp -s 100.64.1.0/24 -d "$PUBB" -j DROP
  ip netns exec nat-b iptables -A FORWARD -p udp -s 100.64.2.0/24 -d "$PUBA" -j DROP
}
udp_block_off() {
  ip netns exec nat-a iptables -D FORWARD -p udp -s 100.64.1.0/24 -d "$PUBB" -j DROP 2>/dev/null || true
  ip netns exec nat-b iptables -D FORWARD -p udp -s 100.64.2.0/24 -d "$PUBA" -j DROP 2>/dev/null || true
}

netem_on() { # delay/loss each way on the subscriber links (defaults 60ms/2%)
  local delay="${1:-${NETEM_DELAY:-60ms}}" loss="${2:-${NETEM_LOSS:-2%}}"
  ip netns exec nat-a tc qdisc add dev a-sub root netem delay "$delay" loss "$loss"
  ip netns exec nat-b tc qdisc add dev b-sub root netem delay "$delay" loss "$loss"
}
netem_off() {
  ip netns exec nat-a tc qdisc del dev a-sub root 2>/dev/null || true
  ip netns exec nat-b tc qdisc del dev b-sub root 2>/dev/null || true
}

derp_block_on() { # make the relay unreachable for both subscribers (TCP 443 to the core)
  for box in nat-a nat-b; do
    ip netns exec "$box" iptables -A FORWARD -p tcp -d "$DERP" --dport 443 -j REJECT
    ip netns exec "$box" iptables -A FORWARD -p tcp -s "$DERP" --sport 443 -j REJECT
  done
}
derp_block_off() {
  for box in nat-a nat-b; do
    ip netns exec "$box" iptables -D FORWARD -p tcp -d "$DERP" --dport 443 -j REJECT 2>/dev/null || true
    ip netns exec "$box" iptables -D FORWARD -p tcp -s "$DERP" --sport 443 -j REJECT 2>/dev/null || true
  done
}

# --- DERP server + derpmap --------------------------------------------------
derper_start() {
  ip netns exec core "$BIN/derper" --hostname=derp.test --certmode=manual --certdir="$CERTS" \
    -a "$DERP:443" --http-port -1 > /tmp/derper.log 2>&1 &
  echo $! > /tmp/derper.pid
  ip netns exec core python3 -u -m http.server 8080 --bind "$DERP" --directory "$WWW" \
    > /tmp/mapserver.log 2>&1 &
  echo $! > /tmp/mapserver.pid
  for _ in $(seq 1 60); do
    ip netns exec core timeout 2 bash -c "(echo > /dev/tcp/$DERP/443)" 2>/dev/null && break
    sleep 0.2
  done
  ip netns exec core timeout 2 bash -c "(echo > /dev/tcp/$DERP/443)" 2>/dev/null || { tail -20 /tmp/derper.log; die "derper não subiu"; }
  for _ in $(seq 1 60); do
    ip netns exec core timeout 2 bash -c "(echo > /dev/tcp/$DERP/8080)" 2>/dev/null && break
    sleep 0.2
  done
  # Fetch through NAT-A: proves the subscriber path, not just core-local.
  ip netns exec pa python3 -c 'import sys,urllib.request; urllib.request.urlopen(sys.argv[1], timeout=5).read()' "$MAPURL" \
    || die "derpmap inacessível via pa (NAT-A)"
}

derper_stop() {
  [ -f /tmp/derper.pid ] && kill "$(cat /tmp/derper.pid)" 2>/dev/null || true
  [ -f /tmp/mapserver.pid ] && kill "$(cat /tmp/mapserver.pid)" 2>/dev/null || true
  rm -f /tmp/derper.pid /tmp/mapserver.pid
}

# --- per-NAT DERP byte counters --------------------------------------------
# Media carried by the relay shows up as TCP :443 traffic on the NAT boxes;
# a working direct path does not. The rules have NO target on purpose: a rule
# without `-j` only counts and falls through, so installing counters never
# overrides an impairment (E7 rejects the same port).
derp_counters_install() {
  for box in nat-a nat-b; do
    ip netns exec "$box" iptables -I FORWARD -p tcp -s "$DERP" --sport 443
    ip netns exec "$box" iptables -I FORWARD -p tcp -d "$DERP" --dport 443
  done
}
derp_counters_read() {
  for box in nat-a nat-b; do
    printf '%s=%s ' "$box" \
      "$(ip netns exec "$box" iptables -L FORWARD -n -v -x 2>/dev/null | awk '/dpt:443|spt:443/ {sum += $2} END {print sum+0}')"
  done
}
derp_counters_remove() {
  for box in nat-a nat-b; do
    ip netns exec "$box" iptables -D FORWARD -p tcp -s "$DERP" --sport 443 2>/dev/null || true
    ip netns exec "$box" iptables -D FORWARD -p tcp -d "$DERP" --dport 443 2>/dev/null || true
  done
}

cleanup_all() {
  derper_stop
  for n in pa pb nat-a nat-b core; do ip netns del "$n" 2>/dev/null || true; done
}
