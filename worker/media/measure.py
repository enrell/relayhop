#!/usr/bin/env python3
"""GStreamer pipeline measurement harness (phase-2 media gate).

Runs a gst-launch-1.0 pipeline with fpsdisplaysink, samples RSS/CPU at 2 Hz,
and prints one JSON summary line. No network, no desktop capture.

Usage:
    GST_PLUGIN_PATH=... python3 measure.py --name NAME --timeout SECS -- <gst-launch args...>

Example:
    python3 measure.py --name x264-ultrafast --timeout 20 -- videotestsrc num-buffers=600 ... !
        x264enc ... ! fpsdisplaysink video-sink=fakesink text-overlay=false
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time


def proc_stat(pid):
    with open(f"/proc/{pid}/stat") as f:
        parts = f.read().rsplit(")", 1)[1].split()
    utime, stime = int(parts[11]), int(parts[12])
    with open("/proc/stat") as f:
        cpu = next(l for l in f if l.startswith("cpu "))
    vals = list(map(int, cpu.split()[1:8]))
    return utime + stime, sum(vals)


def proc_rss_kb(pid):
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--timeout", type=float, default=30)
    ap.add_argument("--expect-frames", type=int, default=0,
                    help="num-buffers of the source; fps is computed as frames/wall")
    ap.add_argument("rest", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    pipeline = args.rest[1:] if args.rest and args.rest[0] == "--" else args.rest

    proc = subprocess.Popen(
        ["gst-launch-1.0", "-q", *pipeline],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1,
    )
    rss_samples, cpu_samples = [], []
    fps_last, rendered, dropped = 0.0, 0, 0
    t0 = time.time()
    try:
        c0, w0 = proc_stat(proc.pid)
    except FileNotFoundError:
        print(json.dumps({"name": args.name, "ok": False, "error": "spawn failed"}))
        return 1
    deadline = t0 + args.timeout
    out = []
    while time.time() < deadline:
        if proc.poll() is not None:
            break
        time.sleep(0.5)
        try:
            rss_samples.append(proc_rss_kb(proc.pid))
            c1, w1 = proc_stat(proc.pid)
            if w1 > w0:
                cpu_samples.append(100.0 * (c1 - c0) / (w1 - w0))
            c0, w0 = c1, w1
        except FileNotFoundError:
            break
    if proc.poll() is None:
        proc.terminate()
    try:
        rest, _ = proc.communicate(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
        rest, _ = proc.communicate()
    for line in (rest or "").splitlines():
        m = re.search(
            r"rendered:\s*(\d+).*dropped:\s*(\d+).*fps:\s*([\d.]+)", line)
        if m:
            rendered, dropped, fps_last = int(m.group(1)), int(m.group(2)), float(m.group(3))
    wall = time.time() - t0
    fps_computed = (args.expect_frames / wall) if args.expect_frames and wall > 0 else 0.0
    try:
        with open("/proc/loadavg") as f:
            loadavg = f.read().split()[:3]
    except OSError:
        loadavg = []
    print(json.dumps({
        "name": args.name,
        "ok": proc.returncode == 0,
        "exit": proc.returncode,
        "wall_s": round(wall, 1),
        "loadavg": loadavg,
        "fps_last": fps_last,
        "fps_computed": round(fps_computed, 1),
        "rendered": rendered or (args.expect_frames if proc.returncode == 0 else 0),
        "dropped": dropped,
        "rss_max_kb": max(rss_samples) if rss_samples else 0,
        "rss_avg_kb": int(sum(rss_samples) / len(rss_samples)) if rss_samples else 0,
        "cpu_avg_pct": round(sum(cpu_samples) / len(cpu_samples), 1) if cpu_samples else 0,
        "cpu_max_pct": round(max(cpu_samples), 1) if cpu_samples else 0,
    }))
    return 0 if proc.returncode == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
