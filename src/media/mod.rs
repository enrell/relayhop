//! Synthetic media pipeline for offline validation.
//!
//! Real capture (Windows Graphics Capture, portal/PipeWire) and hardware
//! encode plug into [`MediaPipeline`] behind these bounds. Until the PLAN.md
//! phase-2 gate passes, only the synthetic source is available and the UI
//! says so.

use std::time::Duration;

use anyhow::{Result, ensure};

use crate::protocol::{VideoCodec, VideoParams, packetize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Synthetic,
    Screen,
    Window,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSource {
    pub id: String,
    pub title: String,
    pub kind: SourceKind,
}

/// Real OS capture is pending the phase-2 gate; the synthetic source lets
/// protocol, transport and session tests run offline.
pub fn list_sources() -> Vec<CaptureSource> {
    vec![CaptureSource {
        id: "synthetic-720p30".into(),
        title: "Fonte de teste (sintética)".into(),
        kind: SourceKind::Synthetic,
    }]
}

pub fn capture_available() -> bool {
    false
}

/// Per-platform capture plan. Only the synthetic source exists today; the
/// OS integrations are tracked here so `diag` reports them honestly.
pub fn capture_plan() -> [(SourceKind, &'static str, &'static str); 3] {
    [
        (
            SourceKind::Synthetic,
            "sintética",
            "disponível: fonte de teste interna, sem acesso ao desktop",
        ),
        (
            SourceKind::Screen,
            "tela",
            "pendente: Windows Graphics Capture / portal ScreenCast + PipeWire",
        ),
        (
            SourceKind::Window,
            "janela",
            "pendente: idem, com seletor e consentimento explícito",
        ),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub seq: u32,
    pub keyframe: bool,
    pub pts_ms: u64,
    pub payload: Vec<u8>,
}

/// Deterministic test-pattern frame sized from the target bitrate.
#[derive(Debug)]
pub struct SyntheticSource {
    params: VideoParams,
    next_seq: u32,
    next_pts_ms: u64,
}

impl SyntheticSource {
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

/// Bounded media pipeline glue: encode (synthetic) -> packetize.
pub struct MediaPipeline {
    source: SyntheticSource,
    pub sent_frames: u64,
    pub sent_bytes: u64,
    pub dropped_frames: u64,
}

impl MediaPipeline {
    pub const QUEUE_CAP: usize = 8;

    pub fn new(params: VideoParams) -> Result<Self> {
        Ok(Self {
            source: SyntheticSource::new(params)?,
            sent_frames: 0,
            sent_bytes: 0,
            dropped_frames: 0,
        })
    }

    pub fn frame_interval(&self) -> Duration {
        self.source.frame_interval()
    }

    /// Produce datagrams for the next frame, or `None` when the bounded
    /// downstream queue is full (counts a drop instead of growing memory).
    pub fn try_next_datagrams(&mut self, queued: usize) -> Result<Option<Vec<Vec<u8>>>> {
        if queued >= Self::QUEUE_CAP {
            // Advance the clock so timing stays honest while shedding load.
            let _ = self.source.next_frame();
            self.dropped_frames += 1;
            return Ok(None);
        }
        let frame = self.source.next_frame();
        let datagrams = packetize(frame.seq, frame.keyframe, frame.pts_ms, &frame.payload)?;
        self.sent_frames += 1;
        self.sent_bytes += frame.payload.len() as u64;
        Ok(Some(datagrams))
    }
}

pub fn supported_codecs() -> &'static [VideoCodec] {
    &[VideoCodec::H264, VideoCodec::Vp8]
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
    fn synthetic_frames_are_bounded_and_periodic() -> Result<()> {
        let params = VideoParams::default();
        let mut pipeline = MediaPipeline::new(params)?;
        let some = pipeline.try_next_datagrams(0)?.expect("datagramas");
        assert!(!some.is_empty());
        assert!(
            pipeline
                .try_next_datagrams(MediaPipeline::QUEUE_CAP)?
                .is_none()
        );
        assert_eq!(pipeline.dropped_frames, 1);
        assert_eq!(list_sources().len(), 1);
        assert!(!capture_available());
        Ok(())
    }
}
