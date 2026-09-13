#!/bin/bash
# 15-minute encoder soak: live 720p30 source -> x264 (zerolatency) -> fakesink.
# Samples RSS/CPU every 10s to series.csv. Pass criteria: completes, no RSS
# growth trend, no EOS errors. Usage: ./soak.sh [outdir] [seconds]
set -u
OUT="${1:-/tmp/opencode/media/soak}"
SECS="${2:-900}"
mkdir -p "$OUT"
cd "$OUT" || exit 1
: "${GST_PLUGIN_PATH:?export GST_PLUGIN_PATH first}"

gst-launch-1.0 -q videotestsrc is-live=true pattern=smpte \
  ! video/x-raw,width=1280,height=720,framerate=30/1 ! videoconvert \
  ! x264enc speed-preset=ultrafast tune=zerolatency bitrate=2500 key-int-max=60 \
  ! h264parse ! mp4mux ! filesink location=soak.mp4 > pipeline.log 2>&1 &
PID=$!
echo "[soak] pid=$PID secs=$SECS"
echo "t_s,rss_kb,cpu_pct,load1" > series.csv
C0=$(awk '{print $14+$15}' /proc/$PID/stat 2>/dev/null || echo 0)
W0=$(awk '{print $1+$2+$3+$4+$5+$6+$7}' /proc/stat | head -1)
for ((t = 10; t <= SECS; t += 10)); do
  sleep 10
  kill -0 "$PID" 2>/dev/null || { echo "[soak] pipeline morreu em t=${t}s"; break; }
  RSS=$(awk '/VmRSS/{print $2}' /proc/$PID/status)
  C1=$(awk '{print $14+$15}' /proc/$PID/stat)
  W1=$(awk '{print $1+$2+$3+$4+$5+$6+$7}' /proc/stat | head -1)
  CPU=$(python3 -c "print(round(100*($C1-$C0)/max(1,($W1-$W0)),1))")
  LOAD=$(cut -d' ' -f1 /proc/loadavg)
  echo "$t,$RSS,$CPU,$LOAD" >> series.csv
  C0=$C1; W0=$W1
done
wait "$PID" 2>/dev/null; RC=$?
# Live sources run forever: stop the pipeline after the sampling window.
if kill -0 "$PID" 2>/dev/null; then
  kill -INT "$PID" 2>/dev/null
  for _ in $(seq 1 15); do kill -0 "$PID" 2>/dev/null || break; sleep 1; done
  kill -KILL "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null; RC=$?
fi
echo "[soak] exit=$RC"
ls -l soak.mp4 2>/dev/null
python3 - "$OUT/series.csv" <<'EOF'
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1])))
if len(rows) < 6:
    print(json.dumps({"name": "soak-x264-720p30", "ok": False, "error": "série curta"}))
    sys.exit(0)
import json
rss = [int(r["rss_kb"]) for r in rows]
n = len(rss)
first = sum(rss[:n//3]) / (n//3)
last = sum(rss[2*n//3:]) / (n - 2*n//3)
print(json.dumps({
    "name": "soak-x264-720p30",
    "ok": True,
    "samples": n,
    "rss_first_kb": int(first),
    "rss_last_kb": int(last),
    "rss_growth_kb": int(last - first),
    "rss_max_kb": max(rss),
}))
EOF
