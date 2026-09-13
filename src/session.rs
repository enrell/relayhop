//! Session supervisor: rooms, participants, local demo and cleanup.
//!
//! Real P2P rendezvous over Tailcat is pending the PLAN.md gate, so remote
//! `watch` fails closed with an actionable error. The offline local demo
//! exercises the full loop (control handshake, synthetic capture ->
//! packetize -> loopback -> reassemble) with bounded queues and cancellation
//! at every step.

use std::{
    fs::{File, OpenOptions},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use directories::ProjectDirs;
use fs2::FileExt;
use tokio::time::{interval, timeout};
use tokio_util::sync::CancellationToken;

use crate::{
    invite::{DEFAULT_INVITE_TTL_SECS, Invite},
    media::MediaPipeline,
    metrics::{Metrics, PathKind, Sample},
    protocol::{Control, FrameDecoder, PROTOCOL_VERSION, Reassembler, VideoParams, encode_control},
    transport::{self, LoopbackEndpoint, loopback_pair},
};

pub const TRANSPORT_UNAVAILABLE: &str = "Transporte P2P real ainda em validação (gate Tailcat/GStreamer pendente). Use Demonstração local ou Diagnóstico.";

/// Fail-closed error for any remote-connect attempt until the gate passes.
pub fn remote_transport_error() -> anyhow::Error {
    anyhow::anyhow!(TRANSPORT_UNAVAILABLE)
}

#[derive(Debug, Clone)]
pub struct Options {
    pub params: VideoParams,
    pub demo_secs: u64,
    pub source_id: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            params: VideoParams::default(),
            demo_secs: 10,
            source_id: "synthetic-720p30".into(),
        }
    }
}

impl Options {
    pub fn validate(&self) -> Result<()> {
        self.params.validate()?;
        crate::media::ensure_supported(self.params.codec)?;
        ensure!(
            (1..=60).contains(&self.demo_secs),
            "demonstração deve durar 1..=60 s"
        );
        ensure!(
            !self.source_id.is_empty() && self.source_id.len() <= 128,
            "fonte inválida"
        );
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Status(String),
    InviteReady { redacted: String },
    Participant(String),
    Metrics(String),
    Finished,
    Error(String),
}

#[derive(Clone, Default)]
pub struct Controls {
    pub stop: CancellationToken,
    pub stop_sharing: CancellationToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomState {
    Idle,
    Preparing,
    Waiting,
    Streaming,
    Stopping,
    Failed,
}

impl RoomState {
    pub fn begin(self) -> Result<Self> {
        ensure!(self == Self::Idle, "sala já iniciada");
        Ok(Self::Preparing)
    }

    pub fn peer_wait(self) -> Result<Self> {
        ensure!(self == Self::Preparing, "transição de sala inválida");
        Ok(Self::Waiting)
    }

    pub fn start_streaming(self) -> Result<Self> {
        ensure!(self == Self::Waiting, "transição de sala inválida");
        Ok(Self::Streaming)
    }

    /// Stopping capture ends sending immediately; the room may stay open.
    pub fn stop_capture(self) -> Result<Self> {
        ensure!(self == Self::Streaming, "nada sendo transmitido");
        Ok(Self::Stopping)
    }

    pub fn fail(self) -> Self {
        Self::Failed
    }

    pub fn finish(self) -> Result<Self> {
        ensure!(
            matches!(self, Self::Waiting | Self::Streaming | Self::Stopping),
            "transição de sala inválida"
        );
        Ok(Self::Idle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantState {
    ValidatingInvite,
    Rendezvous,
    Authenticating,
    Probing,
    Active(PathKind),
    Degraded,
    Recovering,
    Closing,
    Closed,
    Failed,
}

impl ParticipantState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Closed | Self::Failed)
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::ValidatingInvite => "validando convite",
            Self::Rendezvous => "rendezvous",
            Self::Authenticating => "autenticando",
            Self::Probing => "testando caminho",
            Self::Active(path) => match path {
                PathKind::Local => "ativo (local)",
                PathKind::DirectIpv6 => "ativo (direto IPv6)",
                PathKind::DirectIpv4 => "ativo (direto IPv4)",
                PathKind::Relay => "ativo (relay)",
                PathKind::Unknown => "ativo (caminho desconhecido)",
            },
            Self::Degraded => "degradado",
            Self::Recovering => "recuperando",
            Self::Closing => "encerrando",
            Self::Closed => "encerrado",
            Self::Failed => "falha",
        }
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn lock_instance() -> Result<File> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("session.lock"))?;
    lock.try_lock_exclusive()
        .context("Já existe uma sessão do RelayHop em andamento. Use a janela existente.")?;
    Ok(lock)
}

fn data_dir() -> Result<std::path::PathBuf> {
    // Test hook so unit tests never contend with a real user session.
    // Production never sets RELAYHOP_DATA_DIR.
    if let Some(dir) = std::env::var_os("RELAYHOP_DATA_DIR") {
        return Ok(dir.into());
    }
    let dirs = ProjectDirs::from("", "", "relayhop")
        .context("não foi possível localizar os diretórios do usuário")?;
    Ok(dirs.data_local_dir().to_path_buf())
}

/// Create a room invite offline (no network). The full token goes to the
/// caller; only the redacted form is emitted in events.
pub fn prepare_host(options: &Options, now: u64) -> Result<(Invite, RoomState)> {
    options.validate()?;
    let invite = Invite::create(now, DEFAULT_INVITE_TTL_SECS);
    let state = RoomState::Idle.begin()?.peer_wait()?;
    Ok((invite, state))
}

/// Validate an invite token offline (no network, no media yet).
pub fn preview_watch(token: &str, now: u64) -> Result<Invite> {
    Invite::decode(token.trim(), now)
}

/// Control handshake over a reliable ordered channel: Welcome then Hello.
/// Retries once (emitting `Recovering`) before failing closed.
async fn control_handshake(
    host: &mut LoopbackEndpoint,
    viewer: &mut LoopbackEndpoint,
    params: VideoParams,
    room_label: &str,
    stop: &CancellationToken,
    events: &std::sync::mpsc::Sender<Event>,
) -> Result<()> {
    let welcome = encode_control(&Control::Welcome {
        version: PROTOCOL_VERSION,
        params,
    })?;
    let mut attempt = 0;
    loop {
        attempt += 1;
        host.send(&welcome).await?;
        // Viewer side: decode Welcome, validate, answer Hello.
        let received = timeout(Duration::from_secs(2), viewer.recv(stop))
            .await
            .context("rendezvous excedeu o prazo")?
            .context("transporte encerrado no handshake")?;
        let mut decoder = FrameDecoder::new();
        let messages = decoder.push(&received)?;
        let Some(Control::Welcome {
            version,
            params: echoed,
        }) = messages.into_iter().next()
        else {
            bail!("boas-vindas inválidas");
        };
        ensure!(
            version == PROTOCOL_VERSION,
            "versão de protocolo desconhecida"
        );
        echoed.validate()?;
        ensure!(echoed == params, "parâmetros divergentes na negociação");
        let hello = encode_control(&Control::Hello {
            version: PROTOCOL_VERSION,
            room: room_label.into(),
        })?;
        viewer.send(&hello).await?;
        // Host side: decode Hello and check the room label matches.
        let answer = timeout(Duration::from_secs(2), host.recv(stop))
            .await
            .context("resposta excedeu o prazo")?
            .context("transporte encerrado no handshake")?;
        let mut decoder = FrameDecoder::new();
        let messages = decoder.push(&answer)?;
        let Some(Control::Hello { version, room }) = messages.into_iter().next() else {
            if attempt == 1 {
                let _ = events.send(Event::Participant(
                    ParticipantState::Recovering.describe().into(),
                ));
                continue;
            }
            bail!("resposta inválida");
        };
        ensure!(
            version == PROTOCOL_VERSION,
            "versão de protocolo desconhecida"
        );
        ensure!(room == room_label, "sala divergente na autenticação");
        return Ok(());
    }
}

/// Offline end-to-end demo: handshake, synthetic source -> loopback ->
/// reassembler. No public network, no desktop capture, no Tailcat binary.
pub async fn run_local_demo(
    options: Options,
    controls: Controls,
    events: std::sync::mpsc::Sender<Event>,
) -> Result<DemoReport> {
    let result = run_local_demo_inner(&options, &controls, &events).await;
    match &result {
        Ok(_) => {}
        Err(error) => {
            let _ = events.send(Event::Participant(
                ParticipantState::Failed.describe().into(),
            ));
            let _ = events.send(Event::Status(format!("Demonstração falhou: {error:#}")));
        }
    }
    result
}

async fn run_local_demo_inner(
    options: &Options,
    controls: &Controls,
    events: &std::sync::mpsc::Sender<Event>,
) -> Result<DemoReport> {
    let _lock = lock_instance()?;
    options.validate()?;
    let send = |event: Event| {
        let _ = events.send(event);
    };
    let mut room = RoomState::Idle.begin()?.peer_wait()?;
    send(Event::Status("Preparando demonstração local…".into()));
    let invite = Invite::create(now_secs(), DEFAULT_INVITE_TTL_SECS);
    send(Event::InviteReady {
        redacted: invite.redacted(),
    });
    for state in [
        ParticipantState::ValidatingInvite,
        ParticipantState::Rendezvous,
        ParticipantState::Authenticating,
        ParticipantState::Probing,
    ] {
        if controls.stop.is_cancelled() {
            return Ok(DemoReport::cancelled());
        }
        send(Event::Participant(state.describe().into()));
    }

    let (mut host_media, mut viewer_media) = loopback_pair();
    let (mut host_ctl, mut viewer_ctl) = loopback_pair();
    if let Err(error) = control_handshake(
        &mut host_ctl,
        &mut viewer_ctl,
        options.params,
        &invite.redacted(),
        &controls.stop,
        events,
    )
    .await
    {
        room = room.fail();
        debug_assert_eq!(room, RoomState::Failed);
        return Err(error);
    }
    if room == RoomState::Waiting {
        room = room.start_streaming()?;
    }
    let active = ParticipantState::Active(viewer_media.path);
    send(Event::Participant(active.describe().into()));

    let mut pipeline = MediaPipeline::new(options.params)?;
    let mut reassembler = Reassembler::new();
    let mut metrics = Metrics::new();
    let tick = pipeline.frame_interval().max(Duration::from_millis(5));
    let mut ticker = interval(tick);
    let start = Instant::now();
    let budget = Duration::from_secs(options.demo_secs);
    let mut frames = 0u64;
    let mut last_metrics = Instant::now();
    let mut sharing = true;

    loop {
        tokio::select! {
            biased;
            _ = controls.stop.cancelled() => break,
            _ = ticker.tick() => {
                if controls.stop_sharing.is_cancelled() && sharing {
                    sharing = false;
                    if room == RoomState::Streaming {
                        room = room.stop_capture()?;
                    }
                    send(Event::Participant("captura parada pelo transmissor".into()));
                    send(Event::Status("Captura parada; mídia não é mais enviada.".into()));
                }
                if start.elapsed() >= budget {
                    break;
                }
                if sharing
                    && let Some(datagrams) = pipeline.try_next_datagrams(0)?
                {
                    for datagram in datagrams {
                        // Bounded send: abort the demo rather than queue unboundedly.
                        if timeout(Duration::from_millis(200), host_media.send(&datagram))
                            .await
                            .is_err()
                        {
                            bail!("transporte local congestionado");
                        }
                    }
                }
                // Drain everything available without blocking the pacer.
                while let Some(bytes) = viewer_media.try_recv() {
                    let packet = crate::protocol::MediaPacket::decode(&bytes)?;
                    if let Some(frame) = reassembler.insert(packet)? {
                        frames += 1;
                        // Synthetic payload is pattern data; receipt + reassembly is the check.
                        std::hint::black_box(frame.payload.len());
                    }
                }
                if last_metrics.elapsed() >= Duration::from_millis(500) {
                    last_metrics = Instant::now();
                    let elapsed_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                    let sample = Sample {
                        elapsed_ms,
                        bitrate_bps: estimate_bitrate_bps(pipeline.sent_bytes, elapsed_ms),
                        fps: options.params.fps,
                        rtt_ms: 0,
                        path: viewer_media.path,
                        dropped_frames: pipeline.dropped_frames as u32,
                    };
                    metrics.push(sample);
                    if !metrics.is_empty() {
                        send(Event::Metrics(metrics.latest_line()));
                    }
                }
            }
        }
    }

    // Final drain with a short bound so tail frames are counted.
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        let mut progressed = false;
        while let Some(bytes) = viewer_media.try_recv() {
            progressed = true;
            let packet = crate::protocol::MediaPacket::decode(&bytes)?;
            if reassembler.insert(packet)?.is_some() {
                frames += 1;
            }
        }
        if !progressed {
            break;
        }
    }

    let (host_tx_bytes, host_rx_bytes) = host_media.counters();
    let (viewer_tx_bytes, viewer_rx_bytes) = viewer_media.counters();
    debug_assert_eq!(host_rx_bytes, 0);
    debug_assert_eq!(viewer_tx_bytes, 0);
    let (metric_frames, _) = metrics.totals();
    debug_assert_eq!(metric_frames, metrics.len() as u64);
    if pipeline.dropped_frames > 0 || reassembler.dropped() > 0 {
        send(Event::Participant(
            ParticipantState::Degraded.describe().into(),
        ));
    }
    room = room.finish()?;
    let _ = room;
    let report = DemoReport {
        frames,
        reassembled: reassembler.completed(),
        dropped_transport: reassembler.dropped(),
        dropped_pipeline: pipeline.dropped_frames,
        tx_bytes: host_tx_bytes,
        rx_bytes: viewer_rx_bytes,
        avg_bitrate_bps: metrics.avg_bitrate_bps(),
        metric_samples: metrics.len(),
        cancelled: controls.stop.is_cancelled(),
    };
    let closing = ParticipantState::Closing;
    let closed = ParticipantState::Closed;
    debug_assert!(closed.is_terminal());
    debug_assert!(!closing.is_terminal());
    send(Event::Participant(closing.describe().into()));
    send(Event::Participant(closed.describe().into()));
    send(Event::Status(format!(
        "Demonstração local: {frames} frames remontados, {} bytes enviados.",
        report.tx_bytes
    )));
    Ok(report)
}

fn estimate_bitrate_bps(bytes: u64, elapsed_ms: u64) -> u32 {
    if elapsed_ms == 0 {
        return 0;
    }
    let bps = bytes.saturating_mul(8).saturating_mul(1000) / elapsed_ms;
    bps.min(u64::from(u32::MAX)) as u32
}

/// Probe report for `diag`: local binary presence plus offline roundtrips.
/// Never contacts public DERP/STUN or captures the desktop.
pub async fn diagnose() -> Result<String> {
    let mut lines = Vec::new();
    lines.push("RelayHop — diagnóstico local (sem rede pública)".into());
    let params = VideoParams::default();
    params.validate()?;
    lines.push("protocolo: VideoParams padrão válido".into());
    let invite = Invite::create(now_secs(), DEFAULT_INVITE_TTL_SECS);
    let token = invite.encode();
    let parsed = Invite::decode(&token, now_secs())?;
    lines.push(format!("convite: roundtrip OK ({})", parsed.redacted()));
    let (mut a, mut b) = transport::loopback_pair();
    a.send(&[9, 9, 9]).await?;
    let stop = CancellationToken::new();
    let got = timeout(Duration::from_secs(2), b.recv(&stop))
        .await
        .context("loopback sem resposta")?
        .context("par encerrado")?;
    ensure!(got == vec![9, 9, 9], "loopback corrompeu datagrama");
    lines.push("transporte loopback: OK".into());
    for (kind, present, note) in transport::candidate_hints() {
        lines.push(format!(
            "candidato {}: {} ({note})",
            kind.as_str(),
            if present { "presente" } else { "ausente" }
        ));
    }
    match transport::find_on_path() {
        None => lines.push(
            "tailcat: binário não encontrado no PATH (demonstração local não precisa dele)".into(),
        ),
        Some(path) => {
            let availability = transport::probe_tailcat(&path).await;
            lines.push(format!("tailcat: {availability:?}"));
        }
    }
    for (kind, name, status) in crate::media::capture_plan() {
        let _ = kind;
        lines.push(format!("captura {name}: {status}"));
    }
    match crate::display::preserved() {
        Some(vars) => lines.push(format!(
            "ambiente de sessão preservado: {} vars",
            vars.len()
        )),
        None => lines.push("ambiente de sessão preservado: n/a".into()),
    }
    lines.push("rede pública/DERP: não testado neste diagnóstico".into());
    Ok(lines.join("\n"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemoReport {
    pub frames: u64,
    pub reassembled: u64,
    pub dropped_transport: u64,
    pub dropped_pipeline: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub avg_bitrate_bps: u32,
    pub metric_samples: usize,
    pub cancelled: bool,
}

impl DemoReport {
    fn cancelled() -> Self {
        Self {
            frames: 0,
            reassembled: 0,
            dropped_transport: 0,
            dropped_pipeline: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            avg_bitrate_bps: 0,
            metric_samples: 0,
            cancelled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn room_lifecycle_and_capture_stop_semantics() -> Result<()> {
        let idle = RoomState::Idle;
        let waiting = idle.begin()?.peer_wait()?;
        let streaming = waiting.start_streaming()?;
        let stopping = streaming.stop_capture()?;
        assert_eq!(stopping.finish()?, RoomState::Idle);
        assert!(RoomState::Idle.stop_capture().is_err());
        assert_eq!(RoomState::Idle.fail(), RoomState::Failed);
        assert!(!ParticipantState::Active(PathKind::Local).is_terminal());
        assert!(ParticipantState::Closed.is_terminal());
        assert!(ParticipantState::Failed.is_terminal());
        Ok(())
    }

    #[test]
    fn host_preview_roundtrip_offline() -> Result<()> {
        let options = Options::default();
        let (invite, state) = prepare_host(&options, 1_700_000_000)?;
        assert_eq!(state, RoomState::Waiting);
        assert!(!invite.is_expired(1_700_000_000));
        let token = invite.encode();
        let parsed = preview_watch(&token, 1_700_000_000)?;
        assert_eq!(parsed.redacted(), invite.redacted());
        Ok(())
    }

    #[test]
    fn remote_watch_fails_closed_with_actionable_error() {
        let error = remote_transport_error();
        assert!(format!("{error:#}").contains("Demonstração local"));
    }

    #[tokio::test]
    async fn local_demo_reassembles_and_cancels() -> Result<()> {
        // Isolated lock dir: unit tests must not contend with a real session.
        let dir = tempfile::tempdir()?;
        // SAFETY: only this test reads RELAYHOP_DATA_DIR in this process.
        unsafe { std::env::set_var("RELAYHOP_DATA_DIR", dir.path()) };
        let options = Options {
            demo_secs: 1,
            ..Options::default()
        };
        let (tx, rx) = mpsc::channel();
        let controls = Controls::default();
        let report = run_local_demo(options, controls, tx).await?;
        assert!(!report.cancelled);
        assert!(report.frames > 0);
        assert_eq!(report.frames, report.reassembled);
        assert!(report.tx_bytes > 0);
        assert!(report.metric_samples > 0);
        drop(rx);

        // Cancellation before start yields a cancelled report quickly.
        let options = Options {
            demo_secs: 5,
            ..Options::default()
        };
        let (tx, _rx) = mpsc::channel();
        let controls = Controls::default();
        controls.stop.cancel();
        let report = timeout(
            Duration::from_secs(5),
            run_local_demo(options, controls, tx),
        )
        .await??;
        assert!(report.cancelled);
        Ok(())
    }
}
