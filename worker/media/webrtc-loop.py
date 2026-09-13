#!/usr/bin/env python3
"""webrtcbin loopback reference (alternative A): two peers in one process,
SDP/ICE exchanged in-memory. videotestsrc -> x264 -> webrtc-send ->
webrtc-recv -> decode -> fakesink. Counts received frames, prints JSON.

Usage: GST_PLUGIN_PATH=... python3 webrtc-loop.py --frames N --timeout SECS
"""

import argparse
import json
import sys
import time

import gi

gi.require_version("Gst", "1.0")
gi.require_version("GstWebRTC", "1.0")
gi.require_version("GstSdp", "1.0")
from gi.repository import Gst, GstWebRTC, GstSdp, GLib  # noqa: E402

Gst.init(None)


class Peer:
    def __init__(self, name, send):
        pipe_desc = f"webrtcbin name={name} bundle-policy=max-bundle"
        self.pipe = Gst.parse_launch(pipe_desc)
        if self.pipe.get_factory() is not None and self.pipe.get_factory().get_name() == "webrtcbin":
            # Single-element description: parse_launch returns the element itself.
            self.rtc = self.pipe
            wrap = Gst.Pipeline.new(None)
            wrap.add(self.rtc)
            self.pipe = wrap
        else:
            self.rtc = None
            it = self.pipe.iterate_elements()
            while True:
                res, el = it.next()
                if res != Gst.IteratorResult.OK:
                    break
                if el.get_factory() is not None and el.get_factory().get_name() == "webrtcbin":
                    self.rtc = el
                    break
        if self.rtc is None:
            raise RuntimeError("webrtcbin não instanciado")
        self.frames = 0
        if send:
            src = Gst.Bin.new("srcbin")
            elems = [
                ("vs", "videotestsrc", {"is-live": True, "pattern": 0}),
                ("cf", "capsfilter", {"caps": Gst.Caps.from_string(
                    "video/x-raw,width=1280,height=720,framerate=30/1")}),
                ("vc", "videoconvert", {}),
                ("en", "x264enc", {"speed-preset": "ultrafast",
                                   "tune": "zerolatency",
                                   "bitrate": 2500, "key-int-max": 60}),
                ("py", "rtph264pay", {"mtu": 1200}),
                ("rc", "capsfilter", {"caps": Gst.Caps.from_string(
                    "application/x-rtp,encoding-name=H264,clock-rate=90000,payload=96")}),
            ]
            prev = None
            for name, factory, props in elems:
                el = Gst.ElementFactory.make(factory, name)
                if el is None:
                    raise RuntimeError(f"sem elemento {factory}")
                for k, v in props.items():
                    el.set_property(k, v)
                src.add(el)
                if prev is not None:
                    prev.link(el)
                prev = el
            ghost = Gst.GhostPad.new("src", prev.get_static_pad("src"))
            src.add_pad(ghost)
            self.pipe.add(src)
            src.link(self.rtc)
        else:
            self.rtc.connect("pad-added", self.on_pad)
        self.rtc.connect("on-ice-candidate", self.on_ice)

    def on_pad(self, rtc, pad):
        if pad.get_direction() != Gst.PadDirection.SRC:
            return
        decode = Gst.parse_bin_from_description(
            "rtph264depay ! avdec_h264 ! fakesink name=rsink signal-handoffs=true", True
        )
        self.pipe.add(decode)
        decode.sync_state_with_parent()
        sink = decode.get_by_name("rsink")
        sink.connect("handoff", self.on_frame)
        pad.link(decode.get_static_pad("sink"))

    def on_frame(self, sink, buf, pad):
        self.frames += 1

    def on_ice(self, rtc, mline, candidate):
        self.other.rtc.emit("add-ice-candidate", mline, candidate)


def on_offer_created(promise, user_data):
    offerer, answerer = user_data
    reply = promise.get_reply()
    offer = reply.get_value("offer")
    offerer.rtc.emit("set-local-description", offer, None)
    answerer.rtc.emit("set-remote-description", offer, None)
    promise2 = Gst.Promise.new_with_change_func(on_answer_created, (answerer, offerer))
    answerer.rtc.emit("create-answer", None, promise2)


def on_answer_created(promise, user_data):
    answerer, offerer = user_data
    reply = promise.get_reply()
    answer = reply.get_value("answer")
    answerer.rtc.emit("set-local-description", answer, None)
    offerer.rtc.emit("set-remote-description", answer, None)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--frames", type=int, default=300)
    ap.add_argument("--timeout", type=float, default=60)
    args = ap.parse_args()

    send = Peer("send", True)
    recv = Peer("recv", False)
    send.other, recv.other = recv, send
    send.pipe.set_state(Gst.State.PLAYING)
    recv.pipe.set_state(Gst.State.PLAYING)
    promise = Gst.Promise.new_with_change_func(on_offer_created, (send, recv))
    send.rtc.emit("create-offer", None, promise)

    loop = GLib.MainLoop()
    t0 = time.time()
    ctx = loop.get_context()
    for pipe in (send.pipe, recv.pipe):
        bus = pipe.get_bus()
        bus.add_signal_watch()
        def on_msg(bus, msg, label):
            if msg.type == Gst.MessageType.ERROR:
                err, dbg = msg.parse_error()
                print(f"BUS-ERROR[{label}]: {err} | {dbg}", flush=True)
            return True
        lbl = "send" if pipe is send.pipe else "recv"
        bus.connect("message", on_msg, lbl)

    ok = True
    while time.time() - t0 < args.timeout:
        ctx.iteration(False)
        if recv.frames >= args.frames:
            break
        time.sleep(0.02)
    else:
        pass
    if recv.frames < args.frames:
        ok = False
    wall = time.time() - t0
    # drain tail frames briefly
    end = time.time() + 2
    while time.time() < end:
        ctx.iteration(False)
    send.pipe.set_state(Gst.State.NULL)
    recv.pipe.set_state(Gst.State.NULL)
    print(json.dumps({
        "name": "webrtc-loop-h264",
        "ok": ok and recv.frames >= args.frames,
        "wall_s": round(wall, 1),
        "frames_recv": recv.frames,
        "frames_want": args.frames,
    }), flush=True)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
