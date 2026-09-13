//! Local-only session metrics. No telemetry leaves the machine.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    Local,
    DirectIpv6,
    DirectIpv4,
    Relay,
    Unknown,
}

impl PathKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::DirectIpv6 => "direto-ipv6",
            Self::DirectIpv4 => "direto-ipv4",
            Self::Relay => "relay",
            Self::Unknown => "desconhecido",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub elapsed_ms: u64,
    pub bitrate_bps: u32,
    pub fps: u8,
    pub rtt_ms: u16,
    pub path: PathKind,
    pub dropped_frames: u32,
}

#[derive(Debug, Default)]
pub struct Metrics {
    samples: VecDeque<Sample>,
    total_frames: u64,
    total_dropped: u64,
}

impl Metrics {
    pub const CAP: usize = 120;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, sample: Sample) {
        if self.samples.len() >= Self::CAP {
            self.samples.pop_front();
        }
        self.total_frames += 1;
        self.total_dropped += u64::from(sample.dropped_frames);
        self.samples.push_back(sample);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn avg_bitrate_bps(&self) -> u32 {
        if self.samples.is_empty() {
            return 0;
        }
        let sum: u64 = self.samples.iter().map(|s| u64::from(s.bitrate_bps)).sum();
        (sum / self.samples.len() as u64).min(u64::from(u32::MAX)) as u32
    }

    pub fn totals(&self) -> (u64, u64) {
        (self.total_frames, self.total_dropped)
    }

    pub fn latest_line(&self) -> String {
        match self.samples.back() {
            None => "sem métricas ainda".into(),
            Some(sample) => format!(
                "{} · {} kbps · {} fps · rtt {} ms · {} descartados",
                sample.path.as_str(),
                sample.bitrate_bps / 1000,
                sample.fps,
                sample.rtt_ms,
                sample.dropped_frames
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_is_bounded_and_averages() {
        let mut metrics = Metrics::new();
        for i in 0..(Metrics::CAP + 10) {
            metrics.push(Sample {
                elapsed_ms: i as u64,
                bitrate_bps: 1_000_000,
                fps: 30,
                rtt_ms: 20,
                path: PathKind::Local,
                dropped_frames: 0,
            });
        }
        assert_eq!(metrics.len(), Metrics::CAP);
        assert_eq!(metrics.avg_bitrate_bps(), 1_000_000);
        assert!(metrics.latest_line().contains("local"));
    }
}
