//! GStreamer-backed encode (capture -> H.264) and decode (H.264 -> RGBA).
//!
//! Both pipelines live on their own thread: GStreamer is push-based, so the
//! appsink callbacks hand frames to the async supervisor through bounded
//! channels. Nothing here is required for the protocol, the CLI or the offline
//! demo — a build without the `capture` feature still runs those.
//!
//! Encoder: `[videotestsrc | pipewiresrc] ! videoconvert ! <h264 encoder> !
//! h264parse ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink`
//! (one buffer per access unit, so each sample is one decodable frame).
//!
//! Decoder: `appsrc ! h264parse ! <h264 decoder> ! videoconvert !
//! video/x-raw,format=RGBA ! appsink` (latest frame wins; the display never
//! queues more than one image).

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks, AppSrc};

use super::{DecodedImage, EncodedFrame};
use crate::protocol::VideoParams;

/// Encoder preference: hardware first (VA-API, Vulkan Video, NVENC, QSV), then
/// software (x264, openh264). `diag` reports which one was picked.
pub const ENCODER_PREFERENCE: &[&str] = &[
    "vah264enc",
    "vulkanh264enc",
    "nvh264enc",
    "qsvh264enc",
    "x264enc",
    "openh264enc",
];

/// Decoder preference: software first (predictable output format), hardware
/// afterwards. Hardware decoders can emit GL/DMABuf memory that `videoconvert`
/// refuses, so they are only used when nothing else exists.
pub const DECODER_PREFERENCE: &[&str] =
    &["avdec_h264", "openh264dec", "vah264dec", "vulkanh264dec"];

/// Some hardware elements only accept GPU memory, so the pipeline needs an
/// explicit upload/download step around them.
fn upload_element(encoder: &str) -> Option<&'static str> {
    match encoder {
        "vulkanh264enc" => Some("vulkanupload"),
        _ => None,
    }
}

fn download_element(decoder: &str) -> Option<&'static str> {
    match decoder {
        "vulkanh264dec" => Some("vulkandownload"),
        _ => None,
    }
}

/// Encoder plans to try, in order: `(element, optional pre-processing)`.
fn encoder_plans() -> Vec<(&'static str, Option<&'static str>)> {
    available_encoders()
        .into_iter()
        .map(|name| {
            let pre = upload_element(name).filter(|element| has_element(element));
            (name, pre)
        })
        .collect()
}

/// Decoder plans to try, in order: `(element, optional post-processing)`.
fn decoder_plans() -> Vec<(&'static str, Option<&'static str>)> {
    available_decoders()
        .into_iter()
        .map(|name| {
            let post = download_element(name).filter(|element| has_element(element));
            (name, post)
        })
        .collect()
}

/// Bounded hand-off between the GStreamer thread and the supervisor.
const ENCODER_QUEUE: usize = 8;

/// `gst::init()` is idempotent and cheap; call it before touching elements.
pub fn init() -> Result<()> {
    gst::init().map_err(|error| anyhow!("não foi possível iniciar o GStreamer: {error}"))?;
    Ok(())
}

pub fn has_element(name: &str) -> bool {
    // Element factories require an initialized GStreamer; `init()` is
    // idempotent, so probing is always safe.
    if init().is_err() {
        return false;
    }
    gst::ElementFactory::find(name).is_some()
}

fn first_available(preference: &'static [&'static str]) -> Option<&'static str> {
    preference.iter().copied().find(|name| has_element(name))
}

pub fn available_encoders() -> Vec<&'static str> {
    ENCODER_PREFERENCE
        .iter()
        .copied()
        .filter(|name| has_element(name))
        .collect()
}

pub fn available_decoders() -> Vec<&'static str> {
    DECODER_PREFERENCE
        .iter()
        .copied()
        .filter(|name| has_element(name))
        .collect()
}

pub fn selected_encoder() -> Option<&'static str> {
    first_available(ENCODER_PREFERENCE)
}

/// What the encoder captures. Both variants produce the same H.264 stream, so
/// the transport, the protocol and the viewer are identical either way.
#[derive(Debug, Clone)]
pub enum GstInput {
    /// Internal test pattern: real codec, no desktop access, no consent.
    TestPattern,
    /// PipeWire node from the XDG ScreenCast portal (see [`super::portal`]).
    PipeWire { node_id: u32 },
}

impl GstInput {
    fn element(&self) -> Result<gst::Element> {
        match self {
            Self::TestPattern => {
                let src = gst::ElementFactory::make("videotestsrc")
                    .build()
                    .context("videotestsrc indisponível")?;
                src.set_property("is-live", true);
                // Enum properties take their nick, not a string value.
                src.set_property_from_str("pattern", "smpte");
                Ok(src)
            }
            Self::PipeWire { node_id } => {
                let src = gst::ElementFactory::make("pipewiresrc")
                    .build()
                    .context("pipewiresrc indisponível (instale gst-plugin-pipewire)")?;
                src.set_property("path", node_id.to_string());
                src.set_property("do-timestamp", true);
                src.set_property("always-copy", false);
                Ok(src)
            }
        }
    }
}

/// Frame source that encodes with GStreamer.
pub struct GstEncoder {
    frames: Receiver<EncodedFrame>,
    stop: Arc<AtomicBool>,
    /// Pending frame kept when the caller only pulls one per tick.
    pending: Option<EncodedFrame>,
    encoder: Arc<Mutex<Option<gst::Element>>>,
    /// Element actually encoding (filled once the pipeline is running).
    element_slot: Arc<Mutex<Option<&'static str>>>,
    /// Fatal failure reported by the encoder thread (all plans failed, or the
    /// pipeline died mid-stream: revoked capture, device lost).
    error: Arc<Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    interval: Duration,
    pub input: String,
}

impl GstEncoder {
    /// Start encoding. `params` fixes resolution/fps/bitrate before the media
    /// starts (negotiated in the control handshake, never mid-stream).
    pub fn start(input: GstInput, params: VideoParams) -> Result<Self> {
        ensure!(params.fps > 0, "fps inválido");
        init()?;
        let plans = encoder_plans();
        ensure!(
            !plans.is_empty(),
            "nenhum encoder H.264 disponível no GStreamer. Instale gst-plugins-bad \
             (openh264enc/vah264enc), gst-plugins-ugly (x264enc) ou gst-libav, ou rode \
             `relayhop diag` para ver o que falta."
        );
        let (tx, frames) = sync_channel::<EncodedFrame>(ENCODER_QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let encoder_slot: Arc<Mutex<Option<gst::Element>>> = Arc::new(Mutex::new(None));
        let element_slot: Arc<Mutex<Option<&'static str>>> = Arc::new(Mutex::new(None));
        let error_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let input_label = match &input {
            GstInput::TestPattern => "videotestsrc".to_string(),
            GstInput::PipeWire { node_id } => format!("pipewiresrc:{node_id}"),
        };

        let thread = {
            let stop = stop.clone();
            let slot = encoder_slot.clone();
            let element_slot = element_slot.clone();
            let error_slot = error_slot.clone();
            let input = input.clone();
            std::thread::Builder::new()
                .name("relayhop-encode".into())
                .spawn(move || {
                    if let Err(error) = encoder_thread(input, params, tx, stop, slot, element_slot)
                    {
                        tracing::warn!("encoder encerrou: {error:#}");
                        *error_slot.lock().expect("error slot") = Some(format!("{error:#}"));
                    }
                })
                .context("não foi possível criar a thread do encoder")?
        };

        Ok(Self {
            frames,
            stop,
            pending: None,
            encoder: encoder_slot,
            element_slot,
            error: error_slot,
            thread: Some(thread),
            interval: Duration::from_millis(1000 / u64::from(params.fps.max(1))),
            input: input_label,
        })
    }

    /// Fatal failure since the last call, if any (capture revoked, encoder
    /// gone). The supervisor must stop streaming and tell the user.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().expect("error slot").take()
    }

    /// Encoder element in use, once the pipeline reached PLAYING.
    pub fn element(&self) -> Option<&'static str> {
        *self.element_slot.lock().expect("element slot")
    }

    /// Human-readable backend description for status lines.
    pub fn describe(&self) -> String {
        match self.element() {
            Some(element) => format!("{} via {} (H.264)", self.input, element),
            None => format!("{} (H.264, encoder iniciando)", self.input),
        }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Non-blocking pull of the next encoded frame.
    pub fn poll_frame(&mut self) -> Option<EncodedFrame> {
        if self.pending.is_none() {
            self.pending = self.frames.try_recv().ok();
        }
        self.pending.take()
    }

    /// Ask the encoder for an IDR frame by sending the standard upstream
    /// force-key-unit event (GstVideoEncoder handles it for x264, openh264,
    /// VA-API and friends). Returns `false` when the element is not a video
    /// encoder, i.e. when the backend cannot honor the request.
    pub fn force_keyframe(&self) -> bool {
        let guard = self.encoder.lock().expect("encoder slot");
        let Some(element) = guard.as_ref() else {
            return false;
        };
        if element
            .downcast_ref::<gstreamer_video::VideoEncoder>()
            .is_none()
        {
            return false;
        }
        // SAFETY: `gst_video_event_new_upstream_force_key_unit` returns a
        // floating reference owned by us; `from_glib_full` takes ownership.
        // GST_CLOCK_TIME_NONE means "no specific running time".
        let event = unsafe {
            let raw = gstreamer_video::ffi::gst_video_event_new_upstream_force_key_unit(
                gst::ffi::GST_CLOCK_TIME_NONE,
                1,
                0,
            );
            if raw.is_null() {
                return false;
            }
            gst::Event::from_glib_full(raw)
        };
        element.send_event(event)
    }

    /// Stop the pipeline and join the GStreamer thread.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for GstEncoder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn encoder_thread(
    input: GstInput,
    params: VideoParams,
    tx: SyncSender<EncodedFrame>,
    stop: Arc<AtomicBool>,
    slot: Arc<Mutex<Option<gst::Element>>>,
    element_slot: Arc<Mutex<Option<&'static str>>>,
) -> Result<()> {
    init()?;
    let plans = encoder_plans();
    ensure!(!plans.is_empty(), "sem encoder H.264 disponível");
    let mut last_error = None;
    for (name, pre) in plans {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match run_encoder(&input, params, &tx, &stop, &slot, &element_slot, name, pre) {
            Ok(()) => return Ok(()),
            Err(error) => {
                // Hardware encoders can exist but refuse to start (no GPU,
                // no device node): fall back to the next plan instead of
                // ending the session without video.
                tracing::warn!("encoder {name} indisponível: {error:#}");
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("nenhum encoder H.264 conseguiu iniciar")))
}

/// Build and run ONE encoder pipeline. Fails fast (so the caller can try the
/// next plan) when the elements do not link or when no frame arrives within
/// the startup grace period.
#[allow(clippy::too_many_arguments)]
fn run_encoder(
    input: &GstInput,
    params: VideoParams,
    tx: &SyncSender<EncodedFrame>,
    stop: &Arc<AtomicBool>,
    slot: &Arc<Mutex<Option<gst::Element>>>,
    element_slot: &Arc<Mutex<Option<&'static str>>>,
    element_name: &'static str,
    pre: Option<&'static str>,
) -> Result<()> {
    let pipeline = gst::Pipeline::new();
    let src = input.element()?;
    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .context("videoconvert indisponível")?;
    let caps = gst::Caps::builder("video/x-raw")
        .field("width", i32::from(params.width))
        .field("height", i32::from(params.height))
        .field("framerate", gst::Fraction::new(i32::from(params.fps), 1))
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()
        .context("capsfilter indisponível")?;
    let encoder = gst::ElementFactory::make(element_name)
        .build()
        .with_context(|| format!("encoder {element_name} indisponível"))?;
    tune_encoder(&encoder, element_name, params);
    let parser = gst::ElementFactory::make("h264parse")
        .property("config-interval", -1i32)
        .build()
        .context("h264parse indisponível (instale gst-plugins-bad)")?;
    let out_caps = gst::Caps::builder("video/x-h264")
        .field("stream-format", "byte-stream")
        .field("alignment", "au")
        .build();
    let out_filter = gst::ElementFactory::make("capsfilter")
        .property("caps", &out_caps)
        .build()?;
    let sink = gst::ElementFactory::make("appsink")
        .property("sync", false)
        .property("max-buffers", 4u32)
        .property("drop", true)
        .build()
        .context("appsink indisponível")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow!("appsink inesperado"))?;

    let seq = Arc::new(AtomicU64::new(0));
    {
        let seq = seq.clone();
        let tx = tx.clone();
        sink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let Some(buffer) = sample.buffer() else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let keyframe = !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT);
                    let pts_ms = buffer.pts().map(|t| t.mseconds()).unwrap_or(0);
                    let Ok(map) = buffer.map_readable() else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let frame = EncodedFrame {
                        seq: seq.fetch_add(1, Ordering::Relaxed) as u32,
                        keyframe,
                        pts_ms,
                        payload: map.as_slice().to_vec(),
                    };
                    drop(map);
                    match tx.try_send(frame) {
                        Ok(())
                        | Err(TrySendError::Full(_))
                        | Err(TrySendError::Disconnected(_)) => {}
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
    }

    let mut chain: Vec<gst::Element> = vec![src, convert, capsfilter];
    if let Some(pre) = pre {
        chain.push(
            gst::ElementFactory::make(pre)
                .build()
                .with_context(|| format!("pré-processamento {pre} indisponível"))?,
        );
    }
    chain.push(encoder.clone());
    chain.push(parser);
    chain.push(out_filter);
    chain.push(sink.upcast_ref::<gst::Element>().clone());
    pipeline.add_many(chain.iter())?;
    gst::Element::link_many(chain.iter())?;
    *slot.lock().expect("encoder slot") = Some(encoder);
    *element_slot.lock().expect("element slot") = Some(element_name);
    pipeline.set_state(gst::State::Playing)?;

    // Startup grace: a hardware encoder can reach PLAYING and still never
    // produce a buffer (no device). Only after the first frame do we treat
    // the pipeline as healthy.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if seq.load(Ordering::Relaxed) > 0 {
            break;
        }
        if stop.load(Ordering::SeqCst) {
            let _ = pipeline.set_state(gst::State::Null);
            *slot.lock().expect("encoder slot") = None;
            *element_slot.lock().expect("element slot") = None;
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            let _ = pipeline.set_state(gst::State::Null);
            *slot.lock().expect("encoder slot") = None;
            *element_slot.lock().expect("element slot") = None;
            bail!("{element_name} não produziu frames em 5 s");
        }
        if let Some(bus) = pipeline.bus()
            && let Some(message) = bus.timed_pop(Some(gst::ClockTime::from_mseconds(200)))
            && let gst::MessageView::Error(error) = message.view()
        {
            let _ = pipeline.set_state(gst::State::Null);
            *slot.lock().expect("encoder slot") = None;
            *element_slot.lock().expect("element slot") = None;
            bail!("{element_name}: {}", error.error());
        }
    }

    let result = pump_bus(&pipeline, stop, "encode");
    let _ = pipeline.set_state(gst::State::Null);
    *slot.lock().expect("encoder slot") = None;
    *element_slot.lock().expect("element slot") = None;
    result
}

/// Property names and units differ per encoder; keep the mapping explicit.
fn tune_encoder(encoder: &gst::Element, name: &str, params: VideoParams) {
    let kbps = params.bitrate_bps / 1000;
    let gop = u32::from(params.fps) * 2;
    let set_int = |key: &str, value: u32| {
        if encoder.find_property(key).is_some() {
            encoder.set_property_from_value(key, &value.into());
        }
    };
    // Enum properties take their nick, never a string value.
    let set_enum = |key: &str, value: &str| {
        if encoder.find_property(key).is_some() {
            encoder.set_property_from_str(key, value);
        }
    };
    match name {
        "x264enc" => {
            set_int("bitrate", kbps);
            set_enum("speed-preset", "ultrafast");
            set_enum("tune", "zerolatency");
            set_int("key-int-max", gop);
        }
        "openh264enc" => {
            set_int("bitrate", params.bitrate_bps);
            set_enum("complexity", "low");
            set_int("gop-size", gop);
            set_enum("usage-type", "screen");
        }
        "vah264enc" => {
            set_int("bitrate", kbps);
            set_int("key-int-max", gop);
            set_enum("rate-control", "cbr");
        }
        "vulkanh264enc" => {
            // RADV's Vulkan Video encoder accepts `bitrate` but ignores it
            // (measured: identical output in cqp/cbr/vbr and for every
            // `quality` level). The target bitrate is therefore mapped to a
            // constant quantizer derived from bits-per-pixel-per-second; the
            // real rate still depends on the content. See docs/BENCHMARKS.md.
            let bpp = f64::from(params.bitrate_bps)
                / (f64::from(params.width) * f64::from(params.height) * f64::from(params.fps));
            let qp = match bpp {
                b if b >= 0.20 => 28,
                b if b >= 0.12 => 32,
                b if b >= 0.08 => 38,
                b if b >= 0.05 => 43,
                _ => 48,
            };
            set_enum("rate-control", "cqp");
            set_int("qp-i", qp);
            set_int("qp-p", qp);
            set_int("idr-period", gop);
            set_int("bitrate", kbps);
        }
        "nvh264enc" | "qsvh264enc" => {
            set_int("bitrate", kbps);
            set_int("gop-size", gop);
        }
        _ => {
            set_int("bitrate", kbps);
        }
    }
}

/// Run the pipeline's bus until stop/EOS/error.
fn pump_bus(pipeline: &gst::Pipeline, stop: &AtomicBool, what: &str) -> Result<()> {
    let bus = pipeline.bus().context("pipeline sem bus")?;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match bus.timed_pop(Some(gst::ClockTime::from_mseconds(200))) {
            None => continue,
            Some(message) => match message.view() {
                gst::MessageView::Eos(_) => return Ok(()),
                gst::MessageView::Error(error) => {
                    bail!(
                        "{what}: {} ({:?})",
                        error.error(),
                        error.debug().unwrap_or_default()
                    );
                }
                _ => continue,
            },
        }
    }
}

/// Latest decoded frame, shared with the UI. Only the newest image is kept.
pub struct GstDecoder {
    app_src: AppSrc,
    latest: Arc<Mutex<Option<DecodedImage>>>,
    decoded: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    started: bool,
    pub element: &'static str,
}

impl GstDecoder {
    pub fn start() -> Result<Self> {
        init()?;
        let plans = decoder_plans();
        ensure!(
            !plans.is_empty(),
            "nenhum decoder H.264 disponível (instale gst-libav, gst-plugins-bad ou \
             gst-plugins-ugly)"
        );
        let mut last_error = None;
        for (element, post) in plans {
            match Self::build(element, post) {
                Ok(decoder) => return Ok(decoder),
                Err(error) => {
                    tracing::warn!("decoder {element} indisponível: {error:#}");
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow!("nenhum decoder H.264 conseguiu iniciar")))
    }

    fn build(element: &'static str, post: Option<&'static str>) -> Result<Self> {
        let pipeline = gst::Pipeline::new();
        let app_src = gst::ElementFactory::make("appsrc")
            .property("format", gst::Format::Time)
            .property("is-live", false)
            .property("max-bytes", 1u64 << 22)
            .property("block", false)
            .build()
            .context("appsrc indisponível")?
            .downcast::<AppSrc>()
            .map_err(|_| anyhow!("appsrc inesperado"))?;
        let src_caps = gst::Caps::builder("video/x-h264")
            .field("stream-format", "byte-stream")
            .field("alignment", "au")
            .build();
        app_src.set_caps(Some(&src_caps));

        let parser = gst::ElementFactory::make("h264parse").build()?;
        let decoder = gst::ElementFactory::make(element)
            .build()
            .with_context(|| format!("decoder {element} indisponível"))?;
        let convert = gst::ElementFactory::make("videoconvert").build()?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .build();
        let capsfilter = gst::ElementFactory::make("capsfilter")
            .property("caps", &caps)
            .build()?;
        let sink = gst::ElementFactory::make("appsink")
            .property("sync", false)
            .property("max-buffers", 2u32)
            .property("drop", true)
            .build()?
            .downcast::<AppSink>()
            .map_err(|_| anyhow!("appsink inesperado"))?;

        let latest: Arc<Mutex<Option<DecodedImage>>> = Arc::new(Mutex::new(None));
        let decoded = Arc::new(AtomicU64::new(0));
        {
            let latest = latest.clone();
            let decoded = decoded.clone();
            sink.set_callbacks(
                AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        let Some(buffer) = sample.buffer() else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        let Some(caps) = sample.caps() else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        let Some(video) = gstreamer_video::VideoInfo::from_caps(caps).ok() else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        let Ok(map) = buffer.map_readable() else {
                            return Ok(gst::FlowSuccess::Ok);
                        };
                        let width = video.width();
                        let height = video.height();
                        let stride = video.stride()[0] as usize;
                        let row = (width as usize) * 4;
                        let data = map.as_slice();
                        // Tight rows: the egui texture upload has no stride.
                        let mut rgba = Vec::with_capacity(row * height as usize);
                        for y in 0..height as usize {
                            let start = y * stride;
                            let end = start + row;
                            if end > data.len() {
                                break;
                            }
                            rgba.extend_from_slice(&data[start..end]);
                        }
                        drop(map);
                        let image = DecodedImage {
                            width,
                            height,
                            rgba,
                            pts_ms: buffer.pts().map(|t| t.mseconds()).unwrap_or(0),
                        };
                        *latest.lock().expect("latest frame") = Some(image);
                        decoded.fetch_add(1, Ordering::Relaxed);
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            );
        }

        let mut chain: Vec<gst::Element> = vec![
            app_src.upcast_ref::<gst::Element>().clone(),
            parser,
            decoder,
        ];
        if let Some(post) = post {
            chain.push(
                gst::ElementFactory::make(post)
                    .build()
                    .with_context(|| format!("pós-processamento {post} indisponível"))?,
            );
        }
        chain.push(convert);
        chain.push(capsfilter);
        chain.push(sink.upcast_ref::<gst::Element>().clone());
        pipeline.add_many(chain.iter())?;
        gst::Element::link_many(chain.iter())?;
        pipeline.set_state(gst::State::Playing)?;

        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("relayhop-decode".into())
                .spawn(move || {
                    if let Err(error) = pump_bus(&pipeline, &stop, "decode") {
                        tracing::warn!("decoder encerrou: {error:#}");
                    }
                    let _ = pipeline.set_state(gst::State::Null);
                })
                .context("não foi possível criar a thread do decoder")?
        };

        Ok(Self {
            app_src,
            latest,
            decoded,
            dropped,
            stop,
            thread: Some(thread),
            started: false,
            element,
        })
    }

    /// Push one reassembled access unit. Frames before the first keyframe are
    /// dropped: a P-frame without its reference decodes to garbage.
    pub fn push(&mut self, frame: &crate::protocol::AssembledFrame) -> Result<()> {
        if !self.started {
            if !frame.keyframe {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            self.started = true;
        }
        let mut buffer = gst::Buffer::from_slice(frame.payload.clone());
        {
            let buffer = buffer.get_mut().expect("buffer novo");
            buffer.set_pts(gst::ClockTime::from_mseconds(frame.pts_ms));
            buffer.set_dts(gst::ClockTime::NONE);
        }
        match self.app_src.push_buffer(buffer) {
            Ok(_) => Ok(()),
            Err(error) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                bail!("appsrc recusou o frame: {error}")
            }
        }
    }

    /// Newest decoded image, if the UI has not consumed it yet.
    pub fn take_latest(&self) -> Option<DecodedImage> {
        self.latest.lock().expect("latest frame").take()
    }

    pub fn decoded_frames(&self) -> u64 {
        self.decoded.load(Ordering::Relaxed)
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.app_src.end_of_stream();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for GstDecoder {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Human-readable summary for `diag`: what this build can actually do.
pub fn describe() -> Vec<String> {
    if let Err(error) = init() {
        return vec![format!("gstreamer: indisponível ({error})")];
    }
    let encoders = available_encoders();
    let decoders = available_decoders();
    let mut lines = vec![
        format!("gstreamer {} (plugins presentes)", gst::version_string()),
        format!(
            "encoders H.264: {}",
            if encoders.is_empty() {
                "nenhum".to_string()
            } else {
                encoders.join(", ")
            }
        ),
        format!(
            "decoders H.264: {}",
            if decoders.is_empty() {
                "nenhum".to_string()
            } else {
                decoders.join(", ")
            }
        ),
        format!(
            "captura: {} | seletor de tela: {}",
            if has_element("pipewiresrc") {
                "pipewiresrc"
            } else {
                "indisponível"
            },
            if has_element("videotestsrc") {
                "portal XDG + PipeWire (quando o portal estiver disponível)"
            } else {
                "indisponível"
            }
        ),
    ];
    if encoders.is_empty() {
        lines.push(
            "ação: instale gst-plugins-bad (openh264enc/vah264enc) ou gst-plugins-ugly (x264enc)"
                .into(),
        );
    }
    lines
}

/// Encode a fixed number of frames from a synthetic source. Used by tests and
/// by `diag` to prove the media path without a desktop session.
pub fn encode_selftest(params: VideoParams, frames: usize) -> Result<Vec<EncodedFrame>> {
    let mut encoder = GstEncoder::start(GstInput::TestPattern, params)?;
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while out.len() < frames && std::time::Instant::now() < deadline {
        if let Some(frame) = encoder.poll_frame() {
            out.push(frame);
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    encoder.shutdown();
    ensure!(!out.is_empty(), "encoder não produziu frames");
    Ok(out)
}

/// Decode a previously encoded stream (round-trip check for tests/diag).
pub fn decode_selftest(frames: &[EncodedFrame]) -> Result<u64> {
    let mut decoder = GstDecoder::start()?;
    for frame in frames {
        let assembled = crate::protocol::AssembledFrame {
            seq: frame.seq,
            keyframe: frame.keyframe,
            pts_ms: frame.pts_ms,
            payload: frame.payload.clone(),
        };
        decoder.push(&assembled)?;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while decoder.decoded_frames() == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let decoded = decoder.decoded_frames();
    decoder.stop();
    ensure!(decoded > 0, "decoder não produziu imagens");
    Ok(decoded)
}

/// Kept for the UI: the source kinds this build can actually start.
#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip through the real codec: encode a few frames from the test
    /// pattern and decode them again. Skipped when the machine has no encoder
    /// or decoder (the build itself is still valid).
    #[test]
    fn encode_decode_roundtrip_when_available() -> Result<()> {
        init()?;
        if selected_encoder().is_none() || available_decoders().is_empty() {
            eprintln!("sem encoder/decoder H.264 nesta máquina; teste pulado");
            return Ok(());
        }
        let params = VideoParams {
            width: 320,
            height: 240,
            fps: 10,
            bitrate_bps: 500_000,
            ..VideoParams::default()
        };
        let frames = encode_selftest(params, 40)?;
        assert!(frames.iter().any(|frame| frame.keyframe), "sem keyframe");
        assert!(
            frames.iter().all(|frame| !frame.payload.is_empty()),
            "frame vazio"
        );
        // The viewer can only resynchronize if the stream has periodic
        // keyframes; assert it instead of hoping for it.
        let keyframes = frames.iter().filter(|frame| frame.keyframe).count();
        assert!(
            keyframes >= 2,
            "apenas {keyframes} keyframe(s) em {} frames: o espectador não consegue \
             ressincronizar",
            frames.len()
        );
        // `bitrate` must actually reach the encoder (a CQP default ignores it).
        let bytes: usize = frames.iter().map(|frame| frame.payload.len()).sum();
        let seconds = frames.len() as f64 / f64::from(params.fps);
        let kbps = (bytes as f64 * 8.0 / seconds / 1000.0) as u64;
        assert!(
            kbps < 8 * u64::from(params.bitrate_bps) / 1000,
            "bitrate real {kbps} kbps ignorou o alvo de {} kbps",
            params.bitrate_bps / 1000
        );
        let decoded = decode_selftest(&frames)?;
        assert!(decoded > 0);
        Ok(())
    }

    #[test]
    fn describe_lists_backends() -> Result<()> {
        init()?;
        let lines = describe();
        assert!(lines.iter().any(|line| line.contains("encoders H.264")));
        Ok(())
    }

    /// Diagnostic helper (run with `--nocapture`): real bitrate at 720p30.
    #[test]
    fn measure_720p_bitrate_when_available() -> Result<()> {
        init()?;
        if selected_encoder().is_none() {
            return Ok(());
        }
        let params = VideoParams::default();
        let frames = encode_selftest(params, 60)?;
        let bytes: usize = frames.iter().map(|frame| frame.payload.len()).sum();
        let seconds = frames.len() as f64 / f64::from(params.fps);
        let kbps = (bytes as f64 * 8.0 / seconds / 1000.0) as u64;
        let keyframes = frames.iter().filter(|frame| frame.keyframe).count();
        eprintln!(
            "720p30: {kbps} kbps medidos (alvo {} kbps), {keyframes} keyframes em {} frames",
            params.bitrate_bps / 1000,
            frames.len()
        );
        Ok(())
    }
}
