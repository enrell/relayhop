#!/bin/bash
# Phase-2 media gate: encoder budget, RTP loopback, webrtcbin pair,
# PipeWire roundtrip. Appends JSON lines to results.jsonl.
# Usage: GST_PLUGIN_PATH=... ./run-matrix.sh [outdir]
set -u
OUT="${1:-/tmp/opencode/media}"
M="$(cd "$(dirname "$0")" && pwd)/measure.py"
mkdir -p "$OUT"
cd "$OUT" || exit 1
echo "[media] out=$OUT"

SRC="videotestsrc num-buffers=900 pattern=smpte ! video/x-raw,width=1280,height=720,framerate=30/1 ! videoconvert"
run() { python3 "$M" --name "$1" --timeout "$2" --expect-frames "$3" -- $4 >> results.jsonl 2> "$1.err"; tail -1 results.jsonl; }

echo "[media] 1/7 x264 ultrafast"
run enc-x264-ultrafast 120 900 "$SRC ! x264enc speed-preset=ultrafast tune=zerolatency bitrate=2500 key-int-max=60 ! fakesink"
echo "[media] 2/7 x264 veryfast"
run enc-x264-veryfast 180 900 "$SRC ! x264enc speed-preset=veryfast tune=zerolatency bitrate=2500 key-int-max=60 ! fakesink"
echo "[media] 3/7 openh264"
run enc-openh264 180 900 "$SRC ! openh264enc bitrate=2500000 complexity=low ! fakesink"
echo "[media] 4/7 vp8 realtime"
run enc-vp8 240 900 "$SRC ! vp8enc deadline=1 target-bitrate=2500000 keyframe-max-dist=60 ! fakesink"

echo "[media] 5/7 rtp loopback h264 (udpsink/udpsrc, 0% loss esperado)"
bash "$HERE/rtp-loop.sh" 5004 900 > rtp-loop.log 2>&1; tail -2 rtp-loop.log
python3 -c "
import json
log = open('rtp-loop.log').read()
n = int(log.split('FRAMES n=')[1].split()[0])
want = int(log.split('frames-want=')[1].split()[0])
print(json.dumps({'name': 'rtp-h264-loop', 'ok': n >= want,
                  'frames_recv': n, 'frames_want': want,
                  'loss_pct': round(100 * (want - min(n, want)) / want, 1)}))
" | tee -a results.jsonl | tail -1

echo "[media] 6/7 pipewire node visibility (portal capture needs interactive consent)"
gst-launch-1.0 -q videotestsrc pattern=ball ! video/x-raw,width=640,height=480,framerate=30/1 \
  ! pipewiresink stream-properties=props,media.class=Video/Source,node.name=rh-test-src > pw-sink.log 2>&1 &
P4=$!
sleep 3
pw-dump 2>/dev/null | python3 -c "
import json, sys
data = json.load(sys.stdin)
nodes = [(n['id'], n.get('info', {}).get('state')) for n in data
         if n.get('info', {}).get('props', {}).get('node.name') == 'rh-test-src']
ports = [(p['id'], p.get('info', {}).get('direction')) for p in data
         if p.get('type', '').endswith('Port') and p.get('info', {}).get('props', {}).get('node.id') in [i for i, _ in nodes]]
print(json.dumps({'name': 'pipewire-node', 'ok': bool(nodes),
                  'nodes': nodes, 'ports': ports}))
" | tee -a results.jsonl | tail -1
timeout 20 gst-launch-1.0 pipewiresrc target-object=rh-test-src num-buffers=10 ! fakesink > pw-src.log 2>&1
echo "src exit=$? (255 = target-not-found, achado documentado)"
grep -a -o "target not found" pw-src.log | head -1 || echo "src conectou"
kill "$P4" 2>/dev/null; wait "$P4" 2>/dev/null; true

echo "[media] matriz concluída (webrtc-loop e soak rodam separado; ver README)"
