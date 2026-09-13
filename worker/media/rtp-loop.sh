#!/bin/bash
# RTP loopback over UDP loopback: encoder -> udpsink -> udpsrc -> decoder.
# Counts decoded frames with an appsink counter. Usage:
#   GST_PLUGIN_PATH=... ./rtp-loop.sh [port] [frames]
set -u
PORT="${1:-5006}"
FRAMES="${2:-300}"
HERE="$(cd "$(dirname "$0")" && pwd)"

timeout 120 python3 "$HERE/count-sink.py" "$PORT" 90 > /tmp/rtp-count-$$.log 2>&1 &
CNT=$!
sleep 2
timeout 100 gst-launch-1.0 -q videotestsrc num-buffers="$FRAMES" pattern=smpte \
  ! video/x-raw,width=1280,height=720,framerate=30/1 ! videoconvert \
  ! x264enc speed-preset=ultrafast tune=zerolatency bitrate=2500 key-int-max=60 \
  ! rtph264pay mtu=1200 ! udpsink host=127.0.0.1 port="$PORT" > /tmp/rtp-enc-$$.log 2>&1
ENC_RC=$?
wait "$CNT" 2>/dev/null
grep -a FRAMES /tmp/rtp-count-$$.log | tail -1
echo "enc-rc=$ENC_RC frames-want=$FRAMES"
