//! Media: frame sources (capture + encode) and decode for presentation.
//!
//! Two kinds of source exist:
//!
//! - **Real H.264** through GStreamer (feature `capture`, Linux today): either
//!   the internal test pattern (`videotestsrc`) or the desktop obtained from
//!   the XDG ScreenCast portal (`pipewiresrc`, see [`portal`]). Both produce
//!   the same encoded stream, so the transport, the protocol and the viewer do
//!   not care which one is running.
//! - **[`PatternSource`]**: pure-Rust test pattern for unit tests and for
//!   builds without GStreamer. It is deliberately *not* video: it exercises
//!   packetization, reassembly and the session state machine offline.
//!
//! Decoding lives in [`gst::GstDecoder`]; when no decoder is available the
//! viewer still reassembles frames and reports that it could not present them.

use std::time::Duration;

use anyhow::{Result, anyhow, ensure};

use crate::protocol::{VideoCodec, VideoParams, packetize};

#[cfg(all(feature = "capture", target_os = "linux"))]
pub mod gst;
#[cfg(all(feature = "capture", target_os = "linux"))]
pub mod portal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Synthetic,
    Screen,
    Window,
    /// Pure-Rust test pattern (no codec). Tests and `--source pattern`.
    Pattern,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSource {
    pub id: String,
    pub title: String,
    pub kind: SourceKind,
    pub available: bool,
    pub note: String,
}

/// What the encoder should read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaInput {
    /// Internal test pattern through the real encoder.
    Synthetic,
    /// Desktop (screen or window) as a PipeWire node from the portal.
    PipeWire { node_id: u32 },
    /// Pure-Rust pattern: no codec, protocol/session validation only.
    Pattern,
}

impl MediaInput {
    /// Map the `--source`/UI identifier to a concrete input.
    /// `node_id` comes from the portal selection (see [`portal::select_source`]).
    pub fn from_source_id(source_id: &str, node_id: Option<u32>) -> Result<Self> {
        match source_id {
            "synthetic-720p30" | "synthetic" => Ok(Self::Synthetic),
            "pattern" | "pattern-only" => Ok(Self::Pattern),
            "screen" | "window" => match node_id {
                Some(node_id) => Ok(Self::PipeWire { node_id }),
                None => Err(anyhow!(
                    "captura de tela precisa de uma fonte escolhida no seletor do sistema \
                     (portal ScreenCast); nenhum nó PipeWire foi selecionado"
                )),
            },
            other => Err(anyhow!("fonte desconhecida: {other}")),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Synthetic => "padrão de teste interno (H.264 real)".into(),
            Self::PipeWire { node_id } => format!("tela via portal/PipeWire (nó {node_id})"),
            Self::Pattern => "padrão de teste sem codec (apenas protocolo)".into(),
        }
    }
}

/// Encoded frame ready for packetization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub seq: u32,
    pub keyframe: bool,
    pub pts_ms: u64,
    pub payload: Vec<u8>,
}

/// Decoded image for presentation (tight RGBA rows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub pts_ms: u64,
}

/// Any source of encoded frames: real encoder or offline pattern.
pub trait FrameSource: Send {
    /// Non-blocking pull of the next encoded frame.
    fn try_next(&mut self) -> Option<EncodedFrame>;
    fn frame_interval(&self) -> Duration;
    /// Ask for an IDR frame; `false` when the backend cannot force one.
    fn request_keyframe(&mut self) -> bool;
    fn stop(&mut self);
    fn describe(&self) -> String;
    /// Fatal backend failure (capture revoked, encoder died). `None` while the
    /// source is healthy; the supervisor turns it into a visible state change
    /// instead of silently sending nothing.
    fn take_error(&mut self) -> Option<String> {
        None
    }
}

/// Pure-Rust test pattern: deterministic bytes, periodic keyframes, bounded
/// size derived from the target bitrate. No codec is involved.
#[derive(Debug)]
pub struct PatternSource {
    params: VideoParams,
    next_seq: u32,
    next_pts_ms: u64,
}

impl PatternSource {
    pub fn new(params: VideoParams) -> Result<Self> {
        params.validate()?;
        Ok(Self {
            params,
            next_seq: 0,
            next_pts_ms: 0,
        })
    }

    pub fn frame_interval(&self) -> Duration {
        Duration::from_millis(1000 / u64::from(self.params.fps.max(1)))
    }

    pub fn target_frame_bytes(&self) -> usize {
        let per_frame = self.params.bitrate_bps as usize / 8 / self.params.fps as usize;
        per_frame.clamp(256, 65_536)
    }

    pub fn next_frame(&mut self) -> EncodedFrame {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let pts_ms = self.next_pts_ms;
        self.next_pts_ms += 1000 / u64::from(self.params.fps.max(1));
        let size = self.target_frame_bytes();
        let mut payload = Vec::with_capacity(size);
        for i in 0..size {
            payload.push(((seq as usize + i) % 251) as u8);
        }
        EncodedFrame {
            seq,
            keyframe: seq.is_multiple_of(self.params.fps as u32),
            pts_ms,
            payload,
        }
    }
}

impl FrameSource for PatternSource {
    fn try_next(&mut self) -> Option<EncodedFrame> {
        Some(self.next_frame())
    }

    fn frame_interval(&self) -> Duration {
        self.frame_interval()
    }

    fn request_keyframe(&mut self) -> bool {
        false
    }

    fn stop(&mut self) {}

    fn describe(&self) -> String {
        "padrão de teste sem codec (apenas protocolo)".into()
    }
}

#[cfg(all(feature = "capture", target_os = "linux"))]
impl FrameSource for gst::GstEncoder {
    fn try_next(&mut self) -> Option<EncodedFrame> {
        self.poll_frame()
    }

    fn frame_interval(&self) -> Duration {
        self.interval()
    }

    fn request_keyframe(&mut self) -> bool {
        self.force_keyframe()
    }

    fn stop(&mut self) {
        self.shutdown()
    }

    fn describe(&self) -> String {
        gst::GstEncoder::describe(self)
    }

    fn take_error(&mut self) -> Option<String> {
        gst::GstEncoder::take_error(self)
    }
}

/// Bounded media pipeline glue: source -> packetize, with drop accounting.
pub struct MediaPipeline {
    source: Box<dyn FrameSource>,
    pub sent_frames: u64,
    pub sent_bytes: u64,
    pub dropped_frames: u64,
    pub keyframe_requests: u64,
}

impl MediaPipeline {
    pub const QUEUE_CAP: usize = 8;

    /// Build a pipeline for `input`. Fails with an actionable message when the
    /// requested backend is not available in this build/machine.
    pub fn new(params: VideoParams, input: MediaInput) -> Result<Self> {
        params.validate()?;
        ensure_supported(params.codec)?;
        let source: Box<dyn FrameSource> = match &input {
            MediaInput::Pattern => Box::new(PatternSource::new(params)?),
            MediaInput::Synthetic => Self::synthetic_source(params)?,
            MediaInput::PipeWire { node_id } => Self::pipewire_source(params, *node_id)?,
        };
        Ok(Self {
            source,
            sent_frames: 0,
            sent_bytes: 0,
            dropped_frames: 0,
            keyframe_requests: 0,
        })
    }

    /// Backend description, resolved now (the encoder element is only known
    /// after the pipeline reaches PLAYING).
    pub fn backend(&self) -> String {
        self.source.describe()
    }

    #[cfg(all(feature = "capture", target_os = "linux"))]
    fn synthetic_source(params: VideoParams) -> Result<Box<dyn FrameSource>> {
        Ok(Box::new(gst::GstEncoder::start(
            gst::GstInput::TestPattern,
            params,
        )?))
    }

    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    fn synthetic_source(params: VideoParams) -> Result<Box<dyn FrameSource>> {
        // Honest fallback: this build cannot encode. Callers that need real
        // media must say so (see `Options::require_real_media`).
        let _ = params;
        Err(anyhow!(
            "esta compilação não tem captura/encode (feature `capture`); use --source pattern \
             para validar o protocolo sem vídeo"
        ))
    }

    #[cfg(all(feature = "capture", target_os = "linux"))]
    fn pipewire_source(params: VideoParams, node_id: u32) -> Result<Box<dyn FrameSource>> {
        Ok(Box::new(gst::GstEncoder::start(
            gst::GstInput::PipeWire { node_id },
            params,
        )?))
    }

    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    fn pipewire_source(params: VideoParams, node_id: u32) -> Result<Box<dyn FrameSource>> {
        let _ = (params, node_id);
        Err(anyhow!(
            "captura de tela não está disponível nesta plataforma/compilação"
        ))
    }

    pub fn frame_interval(&self) -> Duration {
        self.source.frame_interval()
    }

    /// Produce datagrams for the next frame, or `None` when nothing is ready
    /// or the bounded downstream queue is full (counts a drop instead of
    /// growing memory).
    pub fn try_next_datagrams(&mut self, queued: usize) -> Result<Option<Vec<Vec<u8>>>> {
        if queued >= Self::QUEUE_CAP {
            // Shed load: drop the frame and ask for a keyframe so the viewer
            // can resynchronize instead of decoding against a lost reference.
            if self.source.try_next().is_some() {
                self.dropped_frames += 1;
                self.request_keyframe();
            }
            return Ok(None);
        }
        let Some(frame) = self.source.try_next() else {
            return Ok(None);
        };
        let datagrams = packetize(frame.seq, frame.keyframe, frame.pts_ms, &frame.payload)?;
        self.sent_frames += 1;
        self.sent_bytes += frame.payload.len() as u64;
        Ok(Some(datagrams))
    }

    /// Ask the encoder for an IDR frame (used after loss or congestion).
    pub fn request_keyframe(&mut self) -> bool {
        let forced = self.source.request_keyframe();
        if forced {
            self.keyframe_requests += 1;
        }
        forced
    }

    /// Fatal backend failure since the last call (capture revoked, encoder
    /// died). The caller must stop streaming and tell the user.
    pub fn take_error(&mut self) -> Option<String> {
        self.source.take_error()
    }

    pub fn stop(&mut self) {
        self.source.stop();
    }
}

impl Drop for MediaPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Decoder handle used by the viewer. Portable: when no decoder is available
/// (build without `capture`, or no H.264 decoder installed) the viewer still
/// reassembles frames and reports that it could not present them.
pub struct Decoder {
    #[cfg(all(feature = "capture", target_os = "linux"))]
    inner: Option<gst::GstDecoder>,
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    pushed: u64,
}

#[cfg(all(feature = "capture", target_os = "linux"))]
impl Decoder {
    /// Start a decoder if this build/machine has one; never fails.
    pub fn new() -> Self {
        match gst::GstDecoder::start() {
            Ok(decoder) => Self {
                inner: Some(decoder),
            },
            Err(error) => {
                tracing::warn!("sem decoder de vídeo: {error:#}");
                Self { inner: None }
            }
        }
    }

    pub fn available(&self) -> bool {
        self.inner.is_some()
    }

    /// Backend name for status lines (`None` when there is no decoder).
    pub fn backend(&self) -> Option<&'static str> {
        self.inner.as_ref().map(|decoder| decoder.element)
    }

    /// Push one reassembled access unit. A full queue drops the frame instead
    /// of growing memory; the caller then asks for a keyframe.
    pub fn push(&mut self, frame: &crate::protocol::AssembledFrame) -> Result<()> {
        match self.inner.as_mut() {
            Some(decoder) => decoder.push(frame),
            None => Ok(()),
        }
    }

    pub fn decoded_frames(&self) -> u64 {
        self.inner
            .as_ref()
            .map(|decoder| decoder.decoded_frames())
            .unwrap_or(0)
    }

    pub fn dropped_frames(&self) -> u64 {
        self.inner
            .as_ref()
            .map(|decoder| decoder.dropped_frames())
            .unwrap_or(0)
    }

    /// Newest decoded image, if any arrived since the last call.
    pub fn take_latest(&self) -> Option<DecodedImage> {
        self.inner
            .as_ref()
            .and_then(|decoder| decoder.take_latest())
    }

    pub fn stop(&mut self) {
        if let Some(decoder) = self.inner.as_mut() {
            decoder.stop();
        }
    }
}

#[cfg(not(all(feature = "capture", target_os = "linux")))]
impl Decoder {
    /// No decoder in this build: the viewer still reassembles frames.
    pub fn new() -> Self {
        Self { pushed: 0 }
    }

    pub fn available(&self) -> bool {
        false
    }

    pub fn backend(&self) -> Option<&'static str> {
        None
    }

    pub fn push(&mut self, _frame: &crate::protocol::AssembledFrame) -> Result<()> {
        self.pushed += 1;
        Ok(())
    }

    pub fn decoded_frames(&self) -> u64 {
        0
    }

    pub fn dropped_frames(&self) -> u64 {
        0
    }

    pub fn take_latest(&self) -> Option<DecodedImage> {
        None
    }

    pub fn stop(&mut self) {}
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(not(all(feature = "capture", target_os = "linux")))]
pub mod portal {
    //! Stub for platforms/builds without desktop capture: the UI can still
    //! call the same API and gets an actionable error.

    use std::sync::mpsc::{Receiver, channel};

    use anyhow::Result;

    use super::SourceKind;

    /// Unused placeholder: this build cannot select a capture source.
    #[derive(Debug)]
    pub struct CaptureSelection {
        pub node_id: u32,
        pub label: String,
        pub restore_token: Option<String>,
    }

    pub fn spawn_selection(
        _kind: SourceKind,
        _restore_token: Option<String>,
    ) -> Receiver<Result<CaptureSelection>> {
        let (tx, rx) = channel();
        let _ = tx.send(Err(anyhow::anyhow!(
            "captura de tela não está disponível nesta plataforma/compilação; \
             use a fonte de teste ou o padrão sem codec"
        )));
        rx
    }

    pub fn load_restore_token() -> Option<String> {
        None
    }

    pub fn store_restore_token(_token: &str) {}
}

/// Sources offered by the UI/CLI for this build and machine.
pub fn list_sources() -> Vec<CaptureSource> {
    let mut sources = vec![
        CaptureSource {
            id: "synthetic-720p30".into(),
            title: "Fonte de teste (padrão interno, H.264 real)".into(),
            kind: SourceKind::Synthetic,
            available: capture_available(),
            note: if capture_available() {
                "codifica de verdade pelo GStreamer; não acessa o desktop".into()
            } else {
                "indisponível nesta compilação (sem `capture`)".into()
            },
        },
        CaptureSource {
            id: "pattern".into(),
            title: "Padrão sem codec (apenas protocolo)".into(),
            kind: SourceKind::Pattern,
            available: true,
            note: "sem vídeo real: valida protocolo, transporte e remontagem".into(),
        },
    ];
    for (id, title) in [("screen", "Tela inteira"), ("window", "Janela")] {
        sources.push(CaptureSource {
            id: id.into(),
            title: title.into(),
            kind: if id == "screen" {
                SourceKind::Screen
            } else {
                SourceKind::Window
            },
            available: screen_capture_available(),
            note: if screen_capture_available() {
                "escolha a fonte no seletor do sistema (portal ScreenCast)".into()
            } else {
                "indisponível: exige portal ScreenCast + PipeWire + encoder H.264".into()
            },
        });
    }
    sources
}

/// True when this build/machine can encode real video.
pub fn capture_available() -> bool {
    #[cfg(all(feature = "capture", target_os = "linux"))]
    let available = gst::has_element("videotestsrc") && gst::selected_encoder().is_some();
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    let available = false;
    available
}

/// True when desktop capture can be attempted (portal + PipeWire + encoder).
pub fn screen_capture_available() -> bool {
    #[cfg(all(feature = "capture", target_os = "linux"))]
    let available = capture_available() && gst::has_element("pipewiresrc");
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    let available = false;
    available
}

/// Name of the encoder this build would pick, for honest status lines.
pub fn encoder_name() -> &'static str {
    #[cfg(all(feature = "capture", target_os = "linux"))]
    {
        gst::selected_encoder().unwrap_or("sem encoder")
    }
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    "sem encoder"
}

/// Per-platform capture plan, reported honestly by `diag`.
pub fn capture_plan() -> Vec<(SourceKind, &'static str, String)> {
    let mut plan = vec![
        (
            SourceKind::Synthetic,
            "sintética",
            if capture_available() {
                format!(
                    "disponível: padrão interno codificado em H.264 ({})",
                    encoder_name()
                )
            } else {
                "indisponível: sem encoder H.264 neste build/máquina (rode `relayhop diag`)".into()
            },
        ),
        (
            SourceKind::Pattern,
            "padrão sem codec",
            "disponível: valida protocolo/transporte sem vídeo real".into(),
        ),
    ];
    let (screen_note, window_note) = if screen_capture_available() {
        (
            "disponível: portal ScreenCast escolhe monitor/janela e o PipeWire entrega os frames",
            "disponível: idem, com escolha de janela no seletor do sistema",
        )
    } else {
        (
            "pendente: exige portal ScreenCast + PipeWire + encoder H.264",
            "pendente: exige portal ScreenCast + PipeWire + encoder H.264",
        )
    };
    plan.push((SourceKind::Screen, "tela", screen_note.into()));
    plan.push((SourceKind::Window, "janela", window_note.into()));
    plan
}

pub fn supported_codecs() -> &'static [VideoCodec] {
    &[VideoCodec::H264]
}

pub fn ensure_supported(codec: VideoCodec) -> Result<()> {
    ensure!(
        supported_codecs().contains(&codec),
        "codec ainda não suportado nesta versão"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_frames_are_bounded_and_periodic() -> Result<()> {
        let params = VideoParams::default();
        let mut pipeline = MediaPipeline::new(params, MediaInput::Pattern)?;
        let some = pipeline.try_next_datagrams(0)?.expect("datagramas");
        assert!(!some.is_empty());
        assert!(
            pipeline
                .try_next_datagrams(MediaPipeline::QUEUE_CAP)?
                .is_none()
        );
        assert_eq!(pipeline.dropped_frames, 1);
        assert!(list_sources().iter().any(|s| s.id == "pattern"));
        Ok(())
    }

    #[test]
    fn unknown_source_and_missing_node_fail_closed() {
        assert!(MediaInput::from_source_id("nope", None).is_err());
        assert!(MediaInput::from_source_id("screen", None).is_err());
        assert_eq!(
            MediaInput::from_source_id("screen", Some(7)).expect("nó informado"),
            MediaInput::PipeWire { node_id: 7 }
        );
    }

    #[test]
    fn supported_codec_is_h264_only() {
        assert!(ensure_supported(VideoCodec::H264).is_ok());
        assert!(ensure_supported(VideoCodec::Vp8).is_err());
    }
}
