#!/usr/bin/env python3
"""Assertions for one RelayHop end-to-end scenario (see sim/e2e.sh).

Reads the host/watch stdout (JSON reports), their stderr (participant states,
where the observed transport path is shown), and the DERP byte counters from
the NAT boxes. Prints one RESULT line and appends it to results.jsonl.

Hard assertions are limited to what the application itself controls:
media must actually flow, reassembly must complete, and nothing may leak
secrets into the logs. Path expectations (direct vs relay) are recorded as
`path_expect`/`path_ok` because path selection belongs to the transport
(Tailcat/magicsock), not to this code.
"""

import json
import os
import re
import sys

OUT_DIR = "/tmp/simout"
RESULTS = os.path.join(OUT_DIR, "results.jsonl")

# name -> expectations. `paths=None` means "any path": the choice belongs to the
# transport (Tailcat), not to this code. `min_coverage` is reassembled/sent on
# the viewer side: 0.5 for a link that should deliver, lower where an impairment
# is expected to cost frames.
EXPECT = {
    "E1-lan": {"paths": {"direto IPv4", "direto IPv6", "local"}, "min_coverage": 0.5},
    "E2-cone-nat": {
        "paths": {"relay", "direto IPv4", "direto IPv6"},
        "min_coverage": 0.5,
    },
    "E3-symmetric-nat": {
        "paths": {"relay", "direto IPv4", "direto IPv6"},
        "min_coverage": 0.5,
    },
    # UDP between the peers is dropped: only the relay (TCP) can carry media.
    "E4-udp-blocked": {"paths": {"relay"}, "min_coverage": 0.5},
    # 60 ms + 2% loss each way: the relay's single TCP stream collapses well
    # below the target bitrate, so frames are dropped on purpose. The assertion
    # is "degraded but alive", and the coverage is recorded.
    "E5-lossy": {
        "paths": None,
        "min_coverage": 0.05,
        "drops_ok": True,
        "note": "perda severa: cobertura baixa é o comportamento esperado",
    },
    # 20 ms + 1% loss: mild impairment, must still deliver most frames.
    "E9-mild-loss": {"paths": None, "min_coverage": 0.7, "drops_ok": True},
    "E6-ipv6": {"paths": None, "min_coverage": 0.5, "drops_ok": True},
    "E7-derp-down": {"paths": None, "min_coverage": 0.0, "expect_fail": True},
    "E8-network-change": {"paths": None, "min_coverage": 0.5, "drops_ok": True},
}

PATH_RE = re.compile(r"ativo \((local|direto IPv6|direto IPv4|relay|caminho desconhecido)\)")


def read(path):
    try:
        with open(path, "r", errors="replace") as handle:
            return handle.read()
    except OSError:
        return ""


def report_from(stdout_text):
    """Last JSON object printed by `relayhop host/watch --json`."""
    found = None
    for line in stdout_text.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and value.get("role"):
            found = value
    return found


def paths_in(stderr_text):
    seen = []
    for match in PATH_RE.finditer(stderr_text):
        label = match.group(1)
        if not seen or seen[-1] != label:
            seen.append(label)
    return seen


def counter_pairs(text):
    """`nat-a=123 nat-b=456 ` -> {'nat-a': 123, 'nat-b': 456}"""
    out = {}
    for chunk in text.split():
        if "=" in chunk:
            key, value = chunk.split("=", 1)
            try:
                out[key] = int(value)
            except ValueError:
                out[key] = 0
    return out


def main():
    name = os.environ["SIM_NAME"]
    spec = EXPECT.get(name, {"paths": None, "min_coverage": 0.5, "drops_ok": True})
    allowed_paths = spec.get("paths")
    allow_drops = bool(spec.get("drops_ok", False))
    expect_fail = bool(spec.get("expect_fail", False))
    min_coverage = float(spec.get("min_coverage", 0.5))
    host_out = read(os.environ["SIM_HOST_OUT"])
    host_err = read(os.environ["SIM_HOST_ERR"])
    watch_out = read(os.environ["SIM_WATCH_OUT"])
    watch_err = read(os.environ["SIM_WATCH_ERR"])
    secs = int(os.environ.get("SIM_SECS", "0"))
    wall = int(os.environ.get("SIM_WALL", "0"))
    derp_before = counter_pairs(os.environ.get("SIM_DERP_BEFORE", ""))
    derp_after = counter_pairs(os.environ.get("SIM_DERP_AFTER", ""))

    host = report_from(host_out)
    watch = report_from(watch_out)
    host_paths = paths_in(host_err)
    watch_paths = paths_in(watch_err)
    host_path = host_paths[-1] if host_paths else "none"
    watch_path = watch_paths[-1] if watch_paths else "none"

    failures = []
    notes = []

    if expect_fail:
        # The relay is unreachable: the session must fail with an actionable
        # message instead of hanging forever, and no media may flow.
        if host is not None and host.get("frames", 0) > 0:
            failures.append("host transmitiu com o relay bloqueado")
        if watch is not None and watch.get("reassembled", 0) > 0:
            failures.append("espectador recebeu mídia com o relay bloqueado")
        combined = host_err + watch_err
        actionable = (
            "transporte",
            "worker",
            "prazo",
            "Tailcat",
            "derp",
            "DERP",
            "handshake",
            "dial",
            "conexão",
        )
        if not any(word in combined for word in actionable):
            failures.append(
                f"erro não é acionável: host={host_err[-160:]!r} watch={watch_err[-160:]!r}"
            )
        if wall > secs + 90:
            failures.append(f"demorou demais para falhar ({wall}s)")
    else:
        if host is None:
            failures.append("sem relatório JSON do host")
        if watch is None:
            failures.append("sem relatório JSON do espectador")
        if host is not None:
            if host.get("frames", 0) < 30:
                failures.append(f"host enviou poucos frames: {host.get('frames')}")
            if host.get("tx_bytes", 0) <= 0:
                failures.append("host não enviou bytes")
        if watch is not None:
            if watch.get("reassembled", 0) < 30:
                failures.append(
                    f"espectador remontou poucos frames: {watch.get('reassembled')}"
                )
            if watch.get("rx_bytes", 0) <= 0:
                failures.append("espectador não recebeu bytes")
            # Real codec proof: the viewer must decode the H.264 stream it got
            # over the simulated network, not just count bytes.
            if watch.get("decoded", 0) < 30:
                failures.append(
                    f"espectador decodificou poucos frames: {watch.get('decoded')}"
                )
            bitrate = watch.get("avg_bitrate_bps", 0)
            if not (200_000 <= bitrate <= 4_500_000):
                failures.append(f"bitrate fora da faixa esperada: {bitrate}")
            if not allow_drops and watch.get("dropped_transport", 0) > 0:
                failures.append(
                    f"descartes de transporte sem perda configurada: {watch.get('dropped_transport')}"
                )
        if host is not None and watch is not None:
            sent = host.get("frames", 0)
            got = watch.get("reassembled", 0)
            if sent and got < min_coverage * sent:
                failures.append(
                    f"cobertura baixa: {got}/{sent} frames "
                    f"(mínimo {min_coverage:.0%})"
                )
        if host_path == "none" and watch_path == "none":
            failures.append("nenhum estado de caminho observado nos logs")

    if allowed_paths is not None and watch_path not in allowed_paths:
        failures.append(
            f"caminho observado {watch_path!r} fora do esperado {sorted(allowed_paths)}"
        )

    # Secrets never reach logs: the full token only exists in the host stdout
    # (bearer secret handed to the user) and never in stderr or the reports.
    for label, text in (("host.err", host_err), ("watch.err", watch_err)):
        if "tc" in text and re.search(r"tc[0-9a-f]{40,}", text):
            failures.append(f"{label} vazou endereço de transporte")
    for label, text in (("host.out", host_out), ("watch.out", watch_out)):
        if re.search(r'"psk"', text):
            failures.append(f"{label} contém campo psk")

    derp_bytes = 0
    for key, value in derp_after.items():
        derp_bytes += max(0, value - derp_before.get(key, 0))
    if derp_bytes > 0:
        notes.append(f"relay carregou {derp_bytes} bytes de TCP:443 nos NATs")

    ok = not failures
    coverage = None
    if host and watch and host.get("frames"):
        coverage = round(watch.get("reassembled", 0) / host["frames"], 3)
    result = {
        "name": name,
        "ok": ok,
        "failures": failures,
        "notes": notes,
        "coverage": coverage,
        "min_coverage": min_coverage,
        "host_frames": (host or {}).get("frames"),
        "host_bytes": (host or {}).get("tx_bytes"),
        "watch_frames": (watch or {}).get("reassembled"),
        "watch_decoded": (watch or {}).get("decoded"),
        "watch_backend": (watch or {}).get("backend"),
        "host_backend": (host or {}).get("backend"),
        "watch_bytes": (watch or {}).get("rx_bytes"),
        "watch_bitrate_bps": (watch or {}).get("avg_bitrate_bps"),
        "watch_drops": (watch or {}).get("dropped_transport"),
        "host_path": host_path,
        "watch_path": watch_path,
        "path_expect": sorted(allowed_paths) if allowed_paths else "any",
        "derp_bytes": derp_bytes,
        "wall_secs": wall,
    }
    with open(RESULTS, "a") as handle:
        handle.write(json.dumps(result) + "\n")
    print("RESULT " + json.dumps(result))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
