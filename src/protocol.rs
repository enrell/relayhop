//! Versioned control protocol and bounded media packetization.
//!
//! Control runs over a reliable ordered channel as length-prefixed JSON.
//! Media runs as unreliable datagrams (Tailcat UDP budget: 1232 bytes each).
//! Every decode validates sizes before allocating.

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_CONTROL_BYTES: usize = 64 * 1024;
pub const MAX_FRAME_BYTES: usize = 1_000_000;
pub const MAX_FRAGS_PER_FRAME: usize = 64;
/// Total datagram budget on the Tailcat path (header + payload).
pub const MAX_UDP_DATAGRAM: usize = 1232;
/// Fixed media header size in bytes (magic + fields below).
pub const MEDIA_HEADER_LEN: usize = 20;
/// Payload budget per media datagram.
pub const MAX_MEDIA_PAYLOAD: usize = MAX_UDP_DATAGRAM - MEDIA_HEADER_LEN;
const MEDIA_MAGIC: u16 = 0x5248; // "RH"
const MEDIA_VERSION: u8 = 1;
/// Incremental control decoder buffer cap.
const DECODER_CAP: usize = MAX_CONTROL_BYTES + 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Vp8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoParams {
    pub codec: VideoCodec,
    pub width: u16,
    pub height: u16,
    pub fps: u8,
    pub bitrate_bps: u32,
}

impl VideoParams {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (320..=1920).contains(&self.width),
            "largura fora do intervalo 320..=1920"
        );
        ensure!(
            (240..=1080).contains(&self.height),
            "altura fora do intervalo 240..=1080"
        );
        ensure!((5..=60).contains(&self.fps), "fps fora do intervalo 5..=60");
        ensure!(
            (200_000..=8_000_000).contains(&self.bitrate_bps),
            "bitrate fora do intervalo 200k..8M"
        );
        Ok(())
    }
}

impl Default for VideoParams {
    fn default() -> Self {
        Self {
            codec: VideoCodec::H264,
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_bps: 2_500_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Control {
    Hello { version: u16, room: String },
    Welcome { version: u16, params: VideoParams },
    KeyframeRequest { seq: u32 },
    Bitrate { bps: u32 },
    Ping { id: u32 },
    Pong { id: u32 },
    Bye { reason: String },
}

impl Control {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Hello { version, room } => {
                ensure!(
                    *version == PROTOCOL_VERSION,
                    "versão de protocolo desconhecida"
                );
                ensure!(!room.is_empty() && room.len() <= 64, "sala inválida");
            }
            Self::Welcome { version, params } => {
                ensure!(
                    *version == PROTOCOL_VERSION,
                    "versão de protocolo desconhecida"
                );
                params.validate()?;
            }
            Self::KeyframeRequest { .. } => {}
            Self::Bitrate { bps } => {
                ensure!((50_000..=20_000_000).contains(bps), "bitrate inválido");
            }
            Self::Ping { .. } | Self::Pong { .. } => {}
            Self::Bye { reason } => {
                ensure!(!reason.is_empty() && reason.len() <= 256, "motivo inválido");
            }
        }
        Ok(())
    }
}

/// Encode one control message as u32-BE length + JSON.
pub fn encode_control(message: &Control) -> Result<Vec<u8>> {
    message.validate()?;
    let body = serde_json::to_vec(message)?;
    ensure!(
        body.len() <= MAX_CONTROL_BYTES,
        "mensagem de controle grande demais"
    );
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Incremental decoder for the length-prefixed control stream.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes; returns every complete message decoded so far.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Control>> {
        ensure!(
            self.buffer.len() + bytes.len() <= DECODER_CAP,
            "buffer de controle excedeu o limite"
        );
        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.buffer.len() < 4 {
                break;
            }
            let len = u32::from_be_bytes([
                self.buffer[0],
                self.buffer[1],
                self.buffer[2],
                self.buffer[3],
            ]) as usize;
            ensure!(len <= MAX_CONTROL_BYTES, "frame de controle grande demais");
            if self.buffer.len() < 4 + len {
                break;
            }
            let body: Vec<u8> = self.buffer.drain(..4 + len).collect();
            let message: Control = serde_json::from_slice(&body[4..])?;
            message.validate()?;
            out.push(message);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPacket {
    pub keyframe: bool,
    pub seq: u32,
    pub frag_index: u16,
    pub frag_count: u16,
    pub pts_ms: u64,
    pub payload: Vec<u8>,
}

impl MediaPacket {
    pub fn encode(&self) -> Result<Vec<u8>> {
        ensure!(
            self.frag_count >= 1 && self.frag_count as usize <= MAX_FRAGS_PER_FRAME,
            "contagem de fragmentos inválida"
        );
        ensure!(
            (self.frag_index as usize) < self.frag_count as usize,
            "índice de fragmento inválido"
        );
        ensure!(
            self.payload.len() <= MAX_MEDIA_PAYLOAD,
            "payload excede o orçamento UDP"
        );
        let mut out = Vec::with_capacity(MEDIA_HEADER_LEN + self.payload.len());
        out.extend_from_slice(&MEDIA_MAGIC.to_be_bytes());
        out.push(MEDIA_VERSION);
        out.push(u8::from(self.keyframe));
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.frag_index.to_be_bytes());
        out.extend_from_slice(&self.frag_count.to_be_bytes());
        out.extend_from_slice(&self.pts_ms.to_be_bytes());
        out.extend_from_slice(&self.payload);
        debug_assert_eq!(out.len(), MEDIA_HEADER_LEN + self.payload.len());
        ensure!(out.len() <= MAX_UDP_DATAGRAM, "datagrama excede 1232 bytes");
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= MEDIA_HEADER_LEN, "datagrama curto demais");
        ensure!(bytes.len() <= MAX_UDP_DATAGRAM, "datagrama grande demais");
        let magic = u16::from_be_bytes([bytes[0], bytes[1]]);
        ensure!(magic == MEDIA_MAGIC, "magic inválido");
        ensure!(bytes[2] == MEDIA_VERSION, "versão de mídia desconhecida");
        let keyframe = match bytes[3] {
            0 => false,
            1 => true,
            _ => bail!("flag de keyframe inválida"),
        };
        let seq = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let frag_index = u16::from_be_bytes([bytes[8], bytes[9]]);
        let frag_count = u16::from_be_bytes([bytes[10], bytes[11]]);
        let pts_ms = u64::from_be_bytes([
            bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17], bytes[18], bytes[19],
        ]);
        ensure!(
            frag_count >= 1 && frag_count as usize <= MAX_FRAGS_PER_FRAME,
            "contagem de fragmentos inválida"
        );
        ensure!(
            (frag_index as usize) < frag_count as usize,
            "índice de fragmento inválido"
        );
        let payload = bytes[MEDIA_HEADER_LEN..].to_vec();
        ensure!(payload.len() <= MAX_MEDIA_PAYLOAD, "payload grande demais");
        Ok(Self {
            keyframe,
            seq,
            frag_index,
            frag_count,
            pts_ms,
            payload,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledFrame {
    pub seq: u32,
    pub keyframe: bool,
    pub pts_ms: u64,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
struct InProgress {
    keyframe: bool,
    pts_ms: u64,
    frag_count: usize,
    received: usize,
    parts: Vec<Option<Vec<u8>>>,
    bytes: usize,
}

#[derive(Debug, Default)]
pub struct Reassembler {
    frames: std::collections::BTreeMap<u32, InProgress>,
    order: std::collections::VecDeque<u32>,
    completed: u64,
    dropped: u64,
}

impl Reassembler {
    pub const MAX_IN_PROGRESS: usize = 8;

    pub fn new() -> Self {
        Self::default()
    }

    /// Insert one packet; returns a frame once all its fragments arrive.
    pub fn insert(&mut self, packet: MediaPacket) -> Result<Option<AssembledFrame>> {
        let frag_count = packet.frag_count as usize;
        let frag_index = packet.frag_index as usize;
        if !self.frames.contains_key(&packet.seq) {
            if self.frames.len() >= Self::MAX_IN_PROGRESS
                && let Some(oldest) = self.order.pop_front()
            {
                self.frames.remove(&oldest);
                self.dropped += 1;
            }
            let mut parts = Vec::with_capacity(frag_count);
            parts.resize_with(frag_count, || None);
            self.order.push_back(packet.seq);
            self.frames.insert(
                packet.seq,
                InProgress {
                    keyframe: packet.keyframe,
                    pts_ms: packet.pts_ms,
                    frag_count,
                    received: 0,
                    parts,
                    bytes: 0,
                },
            );
        }
        let Some(entry) = self.frames.get_mut(&packet.seq) else {
            bail!("frame ausente após inserção")
        };
        ensure!(
            entry.frag_count == frag_count
                && entry.keyframe == packet.keyframe
                && entry.pts_ms == packet.pts_ms,
            "fragmento inconsistente com o frame"
        );
        if entry.parts[frag_index].is_none() {
            entry.bytes += packet.payload.len();
            ensure!(entry.bytes <= MAX_FRAME_BYTES, "frame excede 1 MB");
            entry.parts[frag_index] = Some(packet.payload);
            entry.received += 1;
        }
        if entry.received == entry.frag_count {
            let entry = self.frames.remove(&packet.seq).expect("frame presente");
            self.order.retain(|seq| *seq != packet.seq);
            let mut payload = Vec::with_capacity(entry.bytes);
            for part in entry.parts {
                let Some(bytes) = part else {
                    bail!("fragmento ausente na conclusão")
                };
                payload.extend_from_slice(&bytes);
            }
            self.completed += 1;
            return Ok(Some(AssembledFrame {
                seq: packet.seq,
                keyframe: entry.keyframe,
                pts_ms: entry.pts_ms,
                payload,
            }));
        }
        Ok(None)
    }

    pub fn completed(&self) -> u64 {
        self.completed
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// Split an encoded frame into datagram-sized packets.
pub fn packetize(seq: u32, keyframe: bool, pts_ms: u64, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    ensure!(!payload.is_empty(), "frame vazio");
    ensure!(payload.len() <= MAX_FRAME_BYTES, "frame excede 1 MB");
    let frag_count = payload.len().div_ceil(MAX_MEDIA_PAYLOAD);
    ensure!(
        frag_count <= MAX_FRAGS_PER_FRAME,
        "frame precisa de fragmentos demais"
    );
    let mut out = Vec::with_capacity(frag_count);
    for (index, chunk) in payload.chunks(MAX_MEDIA_PAYLOAD).enumerate() {
        let packet = MediaPacket {
            keyframe,
            seq,
            frag_index: index as u16,
            frag_count: frag_count as u16,
            pts_ms,
            payload: chunk.to_vec(),
        };
        out.push(packet.encode()?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_roundtrip_and_bounds() -> Result<()> {
        let message = Control::Welcome {
            version: PROTOCOL_VERSION,
            params: VideoParams::default(),
        };
        let bytes = encode_control(&message)?;
        let mut decoder = FrameDecoder::new();
        // Fragmented delivery must still decode exactly once.
        let first = decoder.push(&bytes[..5])?;
        assert!(first.is_empty());
        let rest = decoder.push(&bytes[5..])?;
        assert_eq!(rest, vec![message]);
        assert!(VideoParams::default().validate().is_ok());
        assert!(
            VideoParams {
                bitrate_bps: 1,
                ..VideoParams::default()
            }
            .validate()
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn control_rejects_bad_version_and_oversize() {
        assert!(
            encode_control(&Control::Hello {
                version: 99,
                room: "x".into(),
            })
            .is_err()
        );
        let mut decoder = FrameDecoder::new();
        // 0x00010001 = MAX_CONTROL_BYTES + 1.
        assert!(decoder.push(&[0, 1, 0, 1]).is_err());
    }

    #[test]
    fn media_split_reassemble_with_reorder_and_duplicates() -> Result<()> {
        let payload: Vec<u8> = (0..5000).map(|i| (i % 251) as u8).collect();
        let mut datagrams = packetize(7, true, 1234, &payload)?;
        assert!(datagrams.iter().all(|d| d.len() <= MAX_UDP_DATAGRAM));
        datagrams.reverse();
        let mut reassembler = Reassembler::new();
        let mut frame = None;
        for bytes in &datagrams {
            let packet = MediaPacket::decode(bytes)?;
            // Duplicate delivery must be idempotent.
            let duplicate = packet.clone();
            if let Some(done) = reassembler.insert(duplicate)? {
                frame = Some(done);
            }
            if let Some(done) = reassembler.insert(packet)? {
                frame = Some(done);
            }
        }
        let frame = frame.expect("frame completo");
        assert_eq!(frame.payload, payload);
        assert!(frame.keyframe);
        assert_eq!(reassembler.completed(), 1);
        Ok(())
    }

    #[test]
    fn media_rejects_inconsistent_or_oversize() -> Result<()> {
        // Two-fragment frame: the second fragment arrives claiming another count.
        let payload = vec![0u8; 2000];
        let datagrams = packetize(1, false, 10, &payload)?;
        assert_eq!(datagrams.len(), 2);
        let mut reassembler = Reassembler::new();
        let first = MediaPacket::decode(&datagrams[0])?;
        assert!(reassembler.insert(first)?.is_none());
        let mut second = MediaPacket::decode(&datagrams[1])?;
        second.frag_count = 63;
        assert!(reassembler.insert(second).is_err());
        assert!(packetize(2, false, 10, &[]).is_err());
        assert!(MediaPacket::decode(&[0u8; 5]).is_err());
        Ok(())
    }
}
