"""Appsink frame counter for the RTP loopback (see rtp-loop.sh).

Usage: count-sink.py PORT SECONDS — prints one FRAMES line.
"""

import sys

import gi

gi.require_version("Gst", "1.0")
from gi.repository import Gst  # noqa: E402

Gst.init(None)

port, budget = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 90
n, first_pts, last_pts = 0, None, None
pipe = Gst.parse_launch(
    f"udpsrc port={port} address=127.0.0.1 timeout=60000000000"
    " ! application/x-rtp,encoding-name=H264,clock-rate=90000"
    " ! rtph264depay ! avdec_h264 ! appsink name=s emit-signals=true"
)
sink = pipe.get_by_name("s")


def on_sample(sink):
    global n, first_pts, last_pts
    sample = sink.emit("pull-sample")
    if sample is None:
        return Gst.FlowReturn.EOS
    n += 1
    pts = sample.get_buffer().pts
    if first_pts is None:
        first_pts = pts
    last_pts = pts
    return Gst.FlowReturn.OK


sink.connect("new-sample", on_sample)
pipe.set_state(Gst.State.PLAYING)
import time

time.sleep(budget)
pipe.set_state(Gst.State.NULL)
print(f"FRAMES n={n} first_pts={first_pts} last_pts={last_pts}", flush=True)
