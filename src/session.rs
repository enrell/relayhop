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
    media::{Decoder, MediaInput, MediaPipeline},
    metrics::{Metrics, PathKind, Sample},
    protocol::{Control, FrameDecoder, PROTOCOL_VERSION, Reassembler, VideoParams, encode_control},
    transport::{
        self, LoopbackEndpoint, loopback_pair,
        tailcat::{self, WorkerEvent, WorkerRx, WorkerTx},
    },
};

pub const DEFAULT_TCP_PORT: u16 = 4106;
pub const DEFAULT_UDP_PORT: u16 = 4107;
/// A capture that produces no frames for this long is treated as failed.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct Options {
    pub params: VideoParams,
    pub demo_secs: u64,
    pub source_id: String,
    /// PipeWire node chosen in the portal picker (screen/window capture).
    pub capture_node: Option<u32>,
    pub derp_map: String,
    pub tcp_port: u16,
    pub udp_port: u16,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            params: VideoParams::default(),
            demo_secs: 10,
            source_id: "synthetic-720p30".into(),
            capture_node: None,
            derp_map: String::new(),
            tcp_port: DEFAULT_TCP_PORT,
            udp_port: DEFAULT_UDP_PORT,
        }
    }
}

impl Options {
    /// Concrete media input for this session (source id + portal selection).
    ///
    /// `RELAYHOP_PIPEWIRE_NODE=<id>` is a test/debug hook: it points the encoder
    /// at an existing PipeWire node instead of opening the portal picker, so the
    /// capture→encode→transport→decode chain can be exercised by scripts. It is
    /// never used by the UI, which always goes through the portal.
    pub fn media_input(&self) -> Result<MediaInput> {
        if let Some(node_id) = debug_pipewire_node() {
            return Ok(MediaInput::PipeWire { node_id });
        }
        MediaInput::from_source_id(&self.source_id, self.capture_node)
    }

    /// Media input for the offline demo: falls back to the pure-Rust pattern
    /// when this build/machine cannot encode, so `local-demo` always runs.
    pub fn demo_media_input(&self) -> MediaInput {
        match self.media_input() {
            Ok(MediaInput::Pattern) => MediaInput::Pattern,
            Ok(_) if crate::media::capture_available() => {
                self.media_input().unwrap_or(MediaInput::Pattern)
            }
            _ => MediaInput::Pattern,
        }
    }
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
        ensure!(self.derp_map.len() <= 512, "URL do mapa DERP grande demais");
        if !self.derp_map.is_empty() {
            ensure!(
                self.derp_map.starts_with("https://") || self.derp_map.starts_with("http://"),
                "mapa DERP deve ser http(s)"
            );
        }
        ensure!(
            self.tcp_port > 0 && self.udp_port > 0 && self.tcp_port != self.udp_port,
            "portas de transporte inválidas"
        );
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Status(String),
    InviteReady {
        redacted: String,
    },
    /// Full bearer token for the sender to copy. NEVER logged: the UI prints
    /// it only in the copy field, the CLI prints it once to stdout.
    InviteToken(String),
    Participant(String),
    Metrics(String),
    Finished,
    Error(String),
}

#[derive(Clone, Default)]
pub struct Controls {
    pub stop: CancellationToken,
    pub stop_sharing: CancellationToken,
    /// Latest decoded frame, shared with the UI on the viewer side. `None` for
    /// the CLI: the supervisor then discards images as they arrive.
    pub view: Option<std::sync::Arc<std::sync::Mutex<Option<crate::media::DecodedImage>>>>,
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

/// Test/debug hook: explicit PipeWire node from the environment (see
/// [`Options::media_input`]). Bounded and never used by the UI.
fn debug_pipewire_node() -> Option<u32> {
    let raw = std::env::var("RELAYHOP_PIPEWIRE_NODE").ok()?;
    let node = raw.trim().parse::<u32>().ok()?;
    (node > 0).then_some(node)
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

    let demo_input = options.demo_media_input();
    let mut pipeline = MediaPipeline::new(options.params, demo_input.clone())?;
    send(Event::Status(format!("vídeo: {}", demo_input.describe())));
    let mut reassembler = Reassembler::new();
    let mut decoder = Decoder::new();
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
                if let Some(error) = pipeline.take_error() {
                    bail!("encode parou: {error}");
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
                        decoder.push(&frame)?;
                    }
                }
                // The demo decodes too, so the codec round-trip is exercised
                // offline (no decoder available -> this is a no-op).
                if let Some(image) = decoder.take_latest() {
                    std::hint::black_box(image.rgba.len());
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
            if let Some(frame) = reassembler.insert(packet)? {
                frames += 1;
                decoder.push(&frame)?;
            }
        }
        if !progressed {
            break;
        }
    }
    let decoded = decoder.decoded_frames();
    let dropped_decode = decoder.dropped_frames();
    decoder.stop();

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
        decoded,
        dropped_decode,
        backend: pipeline.backend(),
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
    #[cfg(all(feature = "capture", target_os = "linux"))]
    {
        lines.extend(crate::media::gst::describe());
        let probe = VideoParams {
            width: 320,
            height: 240,
            fps: 10,
            bitrate_bps: 500_000,
            ..params
        };
        let roundtrip = crate::media::gst::encode_selftest(probe, 6)
            .and_then(|frames| crate::media::gst::decode_selftest(&frames));
        match roundtrip {
            Ok(decoded) => lines.push(format!(
                "codec H.264: encode+decode OK ({decoded} imagens decodificadas no teste)"
            )),
            Err(error) => lines.push(format!("codec H.264: indisponível ({error})")),
        }
        lines.push(format!(
            "portal ScreenCast: {}",
            if tokio::time::timeout(Duration::from_secs(3), crate::media::portal::available())
                .await
                .unwrap_or(false)
            {
                "disponível (a escolha da fonte acontece no seletor do sistema)"
            } else {
                "não respondeu (xdg-desktop-portal rodando?)"
            }
        ));
    }
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    lines.push("captura real: não compilada nesta plataforma (feature `capture`)".into());
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

/// Cancellable await: every network wait competes with `stop`.
async fn cancel_aware<T>(
    stop: &CancellationToken,
    fut: impl std::future::Future<Output = T>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = stop.cancelled() => bail!("encerrado"),
        out = fut => Ok(out),
    }
}

async fn wait_worker_event(
    events: &mut tokio::sync::mpsc::Receiver<WorkerEvent>,
    stop: &CancellationToken,
    secs: u64,
    what: &str,
) -> Result<WorkerEvent> {
    let event = cancel_aware(stop, timeout(Duration::from_secs(secs), events.recv()))
        .await?
        .with_context(|| format!("{what} excedeu o prazo"))?
        .context("worker encerrou")?;
    match &event {
        WorkerEvent::Closed { reason } => bail!("worker encerrou: {reason}"),
        WorkerEvent::Error(error) => bail!("worker: {error}"),
        _ => Ok(event),
    }
}

/// Read stream chunks until at least one control message decodes.
/// Returns all messages decoded from the latest chunk (a TCP segment can
/// coalesce Hello + Link); callers must process every item.
async fn next_control(
    control: &mut tokio::sync::mpsc::Receiver<Vec<u8>>,
    decoder: &mut FrameDecoder,
    stop: &CancellationToken,
) -> Result<Vec<Control>> {
    loop {
        let chunk = cancel_aware(stop, timeout(Duration::from_secs(30), control.recv()))
            .await?
            .context("leitura de controle excedeu o prazo")?
            .context("controle encerrado")?;
        let messages: Vec<Control> = decoder
            .push(&chunk)?
            .into_iter()
            .map(|message| match message {
                Control::Bye { reason } => Err(anyhow::anyhow!("encerrado pelo par: {reason}")),
                other => Ok(other),
            })
            .collect::<Result<_>>()?;
        if !messages.is_empty() {
            return Ok(messages);
        }
    }
}

/// Remote host: serve over Tailcat, publish an invite carrying the rendezvous
/// address, handshake, then stream synthetic media until stopped.
pub async fn run_host(
    options: Options,
    controls: Controls,
    events: std::sync::mpsc::Sender<Event>,
) -> Result<DemoReport> {
    let _lock = lock_instance()?;
    options.validate()?;
    let send = |event: Event| {
        let _ = events.send(event);
    };
    let mut room = RoomState::Idle.begin()?.peer_wait()?;
    send(Event::Status("Iniciando transporte P2P…".into()));
    let worker = cancel_aware(
        &controls.stop,
        tailcat::Worker::serve(
            options.tcp_port,
            options.udp_port,
            &options.derp_map,
            &controls.stop,
        ),
    )
    .await??;
    let (mut tx, rx) = worker.split();
    let result = run_host_inner(
        options,
        controls.clone(),
        events.clone(),
        &mut tx,
        rx,
        &mut room,
    )
    .await;
    // Graceful worker shutdown (F_CLOSE + EOF + wait); drop kills on failure.
    let _ = tx.stop().await;
    send(Event::Participant(
        ParticipantState::Closing.describe().into(),
    ));
    let report = result?;
    send(Event::Participant(
        ParticipantState::Closed.describe().into(),
    ));
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
async fn run_host_inner(
    options: Options,
    controls: Controls,
    events: std::sync::mpsc::Sender<Event>,
    tx: &mut WorkerTx,
    rx: WorkerRx,
    room: &mut RoomState,
) -> Result<DemoReport> {
    let WorkerRx {
        mut control_rx,
        media_rx: _,
        events: mut worker_events,
    } = rx;
    let send = |event: Event| {
        let _ = events.send(event);
    };
    let addr = match wait_worker_event(
        &mut worker_events,
        &controls.stop,
        tailcat::READY_TIMEOUT_SECS,
        "transporte",
    )
    .await?
    {
        WorkerEvent::Ready { addr } => addr,
        _ => bail!("transporte não ficou pronto"),
    };
    let invite = Invite::create(now_secs(), DEFAULT_INVITE_TTL_SECS).with_transport(&addr)?;
    // The full token is bearer-secret: exactly one event carries it, and no
    // log/status line ever prints it (see InviteToken docs).
    send(Event::InviteReady {
        redacted: invite.redacted(),
    });
    send(Event::InviteToken(invite.encode()));
    send(Event::Status(format!(
        "Sala aberta (expira em ~{} min). Envie o convite ao espectador.",
        invite.expires_at().saturating_sub(now_secs()) / 60
    )));
    loop {
        match wait_worker_event(
            &mut worker_events,
            &controls.stop,
            tailcat::PEER_TIMEOUT_SECS,
            "espectador",
        )
        .await?
        {
            WorkerEvent::Peer => break,
            WorkerEvent::Path(_) => continue,
            _ => bail!("transporte não conectou"),
        }
    }
    send(Event::Participant(
        ParticipantState::Authenticating.describe().into(),
    ));
    tx.send_control(&encode_control(&Control::Welcome {
        version: PROTOCOL_VERSION,
        params: options.params,
    })?)
    .await?;
    // Expect Hello, then Link (viewer reports its observed path).
    let mut decoder = FrameDecoder::new();
    let handshake_end = Instant::now() + Duration::from_secs(90);
    let mut peer_path = loop {
        ensure!(Instant::now() < handshake_end, "handshake excedeu o prazo");
        let mut found = None;
        for message in next_control(&mut control_rx, &mut decoder, &controls.stop).await? {
            match message {
                Control::Hello {
                    version,
                    room: label,
                } => {
                    ensure!(
                        version == PROTOCOL_VERSION,
                        "versão de protocolo desconhecida"
                    );
                    ensure!(
                        label == invite.redacted(),
                        "sala divergente na autenticação"
                    );
                }
                Control::Link { path } => {
                    found = Some(path);
                    break;
                }
                _ => {}
            }
        }
        if let Some(path) = found {
            break path;
        }
    };
    if *room == RoomState::Waiting {
        *room = room.start_streaming()?;
    }
    send(Event::Participant(
        ParticipantState::Active(peer_path).describe().into(),
    ));

    let media_input = options.media_input()?;
    let mut pipeline = MediaPipeline::new(options.params, media_input.clone())?;
    send(Event::Status(format!("vídeo: {}", media_input.describe())));
    let mut metrics = Metrics::new();
    let tick = pipeline.frame_interval().max(Duration::from_millis(5));
    let mut ticker = interval(tick);
    let start = Instant::now();
    let mut last_frame_at = Instant::now();
    let mut last_metrics = Instant::now();
    let mut decoder = FrameDecoder::new();
    loop {
        tokio::select! {
            biased;
            _ = controls.stop.cancelled() => break,
            event = worker_events.recv() => {
                match event {
                    Some(WorkerEvent::Path(path)) => {
                        peer_path = path;
                        send(Event::Participant(ParticipantState::Active(path).describe().into()));
                    }
                    Some(WorkerEvent::Closed { reason }) => bail!("worker encerrou: {reason}"),
                    Some(WorkerEvent::Error(error)) => bail!("worker: {error}"),
                    None => bail!("worker encerrou"),
                    _ => {}
                }
            }
            chunk = control_rx.recv() => {
                let Some(chunk) = chunk else {
                    // The viewer's control stream is gone: end the session
                    // cleanly (the room closes) instead of reporting an error.
                    send(Event::Status("Espectador desconectou; encerrando a sessão.".into()));
                    break;
                };
                let mut left = false;
                for message in decoder.push(&chunk)? {
                    match message {
                        Control::Bye { reason } => {
                            send(Event::Status(format!("Espectador encerrou: {reason}")));
                            left = true;
                        }
                        Control::Link { path } => {
                            peer_path = path;
                            send(Event::Participant(ParticipantState::Active(path).describe().into()));
                        }
                        Control::KeyframeRequest { .. } => {
                            // The viewer lost a reference: force an IDR so it can
                            // resynchronize instead of showing artifacts.
                            let forced = pipeline.request_keyframe();
                            tracing::debug!(forced, "pedido de keyframe do espectador");
                        }
                        _ => {}
                    }
                }
                if left {
                    break;
                }
            }
            _ = ticker.tick() => {
                if controls.stop_sharing.is_cancelled() {
                    if *room == RoomState::Streaming {
                        *room = room.stop_capture()?;
                    }
                    let _ = tx.send_control(&encode_control(&Control::Bye { reason: "captura parada".into() })?).await;
                    send(Event::Participant("captura parada pelo transmissor".into()));
                    break;
                }
                // A dead encoder (revoked screen cast, lost device) must end
                // the session instead of streaming silence forever.
                if let Some(error) = pipeline.take_error() {
                    send(Event::Status(format!("captura/encode parou: {error}")));
                    let _ = tx
                        .send_control(&encode_control(&Control::Bye {
                            reason: "captura encerrada".into(),
                        })?)
                        .await;
                    break;
                }
                if let Some(datagrams) = pipeline.try_next_datagrams(0)? {
                    last_frame_at = Instant::now();
                    for datagram in datagrams {
                        tx.send_media(&datagram).await?;
                    }
                } else if last_frame_at.elapsed() > STALL_TIMEOUT {
                    // Bounded watchdog: capture that produces nothing is a
                    // failure the viewer must see, not a silent freeze.
                    send(Event::Status(
                        "captura não produziu frames; encerrando a transmissão".into(),
                    ));
                    let _ = tx
                        .send_control(&encode_control(&Control::Bye {
                            reason: "captura parada".into(),
                        })?)
                        .await;
                    break;
                }
                if last_metrics.elapsed() >= Duration::from_millis(500) {
                    last_metrics = Instant::now();
                    let elapsed_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                    metrics.push(Sample {
                        elapsed_ms,
                        bitrate_bps: estimate_bitrate_bps(pipeline.sent_bytes, elapsed_ms),
                        fps: options.params.fps,
                        rtt_ms: 0,
                        path: peer_path,
                        dropped_frames: pipeline.dropped_frames as u32,
                    });
                    send(Event::Metrics(metrics.latest_line()));
                }
            }
        }
    }
    let _ = tx
        .send_control(&encode_control(&Control::Bye {
            reason: "sessão encerrada".into(),
        })?)
        .await;
    pipeline.stop();
    *room = room.finish()?;
    let report = DemoReport {
        frames: pipeline.sent_frames,
        reassembled: 0,
        dropped_transport: 0,
        dropped_pipeline: pipeline.dropped_frames,
        decoded: 0,
        dropped_decode: 0,
        backend: pipeline.backend(),
        tx_bytes: pipeline.sent_bytes,
        rx_bytes: 0,
        avg_bitrate_bps: metrics.avg_bitrate_bps(),
        metric_samples: metrics.len(),
        cancelled: controls.stop.is_cancelled(),
    };
    // Graceful worker shutdown (F_CLOSE + EOF + wait); drop kills on failure.
    let _ = tx.stop().await;
    Ok(report)
}

/// Remote watch: dial the invite's transport address, handshake, receive and
/// reassemble media until stopped or the host ends the session.
pub async fn run_watch(
    token: &str,
    options: Options,
    controls: Controls,
    events: std::sync::mpsc::Sender<Event>,
) -> Result<DemoReport> {
    let _lock = lock_instance()?;
    options.validate()?;
    let send = |event: Event| {
        let _ = events.send(event);
    };
    send(Event::Participant(
        ParticipantState::ValidatingInvite.describe().into(),
    ));
    let invite = Invite::decode(token.trim(), now_secs())?;
    let addr = invite.transport().context(
        "Este convite não tem endereço de transporte (gerado antes do modo P2P). Gere um novo convite.",
    )?;
    send(Event::Participant(
        ParticipantState::Rendezvous.describe().into(),
    ));
    let worker = cancel_aware(
        &controls.stop,
        tailcat::Worker::join(
            addr,
            options.tcp_port,
            options.udp_port,
            &options.derp_map,
            &controls.stop,
        ),
    )
    .await??;
    let (mut tx, rx) = worker.split();
    let result = run_watch_inner(
        options,
        controls.clone(),
        events.clone(),
        &mut tx,
        rx,
        &invite,
    )
    .await;
    // Graceful worker shutdown (F_CLOSE + EOF + wait); drop kills on failure.
    let _ = tx.stop().await;
    send(Event::Participant(
        ParticipantState::Closing.describe().into(),
    ));
    let report = result?;
    send(Event::Participant(
        ParticipantState::Closed.describe().into(),
    ));
    Ok(report)
}

async fn run_watch_inner(
    _options: Options,
    controls: Controls,
    events: std::sync::mpsc::Sender<Event>,
    tx: &mut WorkerTx,
    rx: WorkerRx,
    invite: &Invite,
) -> Result<DemoReport> {
    let WorkerRx {
        mut control_rx,
        mut media_rx,
        events: mut worker_events,
    } = rx;
    let send = |event: Event| {
        let _ = events.send(event);
    };
    loop {
        match wait_worker_event(
            &mut worker_events,
            &controls.stop,
            tailcat::PEER_TIMEOUT_SECS,
            "conexão",
        )
        .await?
        {
            WorkerEvent::Peer => break,
            WorkerEvent::Path(_) => continue,
            _ => bail!("transporte não conectou"),
        }
    }
    send(Event::Participant(
        ParticipantState::Authenticating.describe().into(),
    ));
    let mut decoder = FrameDecoder::new();
    let params = loop {
        let mut found = None;
        for message in next_control(&mut control_rx, &mut decoder, &controls.stop).await? {
            if let Control::Welcome { version, params } = message {
                ensure!(
                    version == PROTOCOL_VERSION,
                    "versão de protocolo desconhecida"
                );
                params.validate()?;
                found = Some(params);
                break;
            }
        }
        if let Some(params) = found {
            break params;
        }
    };
    params.validate()?;
    params.validate()?;
    tx.send_control(&encode_control(&Control::Hello {
        version: PROTOCOL_VERSION,
        room: invite.redacted(),
    })?)
    .await?;
    // Report our observed path immediately (Unknown at first), then again
    // whenever the worker refines it. The host displays the latest Link.
    let mut own_path = PathKind::Unknown;
    tx.send_control(&encode_control(&Control::Link { path: own_path })?)
        .await?;
    send(Event::Participant(
        ParticipantState::Probing.describe().into(),
    ));
    send(Event::Participant(
        ParticipantState::Active(own_path).describe().into(),
    ));

    let mut reassembler = Reassembler::new();
    let mut metrics = Metrics::new();
    let mut video_decoder = Decoder::new();
    if video_decoder.available() {
        send(Event::Status(format!(
            "decodificador: {}",
            video_decoder.backend().unwrap_or("desconhecido")
        )));
    } else {
        send(Event::Status(
            "sem decodificador H.264: os frames são remontados mas não exibidos".into(),
        ));
    }
    let start = Instant::now();
    let mut frames = 0u64;
    let mut rx_bytes = 0u64;
    let mut last_seq: Option<u32> = None;
    let mut keyframe_requests = 0u64;
    let mut last_metrics = Instant::now();
    let mut decoder = FrameDecoder::new();
    loop {
        tokio::select! {
            biased;
            _ = controls.stop.cancelled() => {
                let _ = tx.send_control(&encode_control(&Control::Bye { reason: "espectador saiu".into() })?).await;
                break;
            }
            event = worker_events.recv() => {
                match event {
                    Some(WorkerEvent::Path(path)) => {
                        own_path = path;
                        let _ = tx.send_control(&encode_control(&Control::Link { path })?).await;
                        send(Event::Participant(ParticipantState::Active(path).describe().into()));
                    }
                    Some(WorkerEvent::Closed { reason }) => bail!("worker encerrou: {reason}"),
                    Some(WorkerEvent::Error(error)) => bail!("worker: {error}"),
                    None => bail!("worker encerrou"),
                    _ => {}
                }
            }
            chunk = control_rx.recv() => {
                let Some(chunk) = chunk else { bail!("controle encerrado") };
                for message in decoder.push(&chunk)? {
                    if let Control::Bye { .. } = message {
                        send(Event::Status("Transmissor encerrou a sessão.".into()));
                        return finish_watch(
                            metrics,
                            reassembler,
                            frames,
                            rx_bytes,
                            video_decoder.decoded_frames(),
                            video_decoder.dropped_frames(),
                            video_decoder.backend().unwrap_or("nenhum").to_string(),
                            &controls,
                        );
                    }
                }
            }
            datagram = media_rx.recv() => {
                let Some(bytes) = datagram else { break };
                rx_bytes += bytes.len() as u64;
                let packet = crate::protocol::MediaPacket::decode(&bytes)?;
                if let Some(frame) = reassembler.insert(packet)? {
                    frames += 1;
                    // A gap in sequence numbers means a reference frame was
                    // lost: ask the encoder for an IDR instead of decoding
                    // against a missing reference.
                    if let Some(previous) = last_seq
                        && frame.seq != previous.wrapping_add(1)
                    {
                        let _ = tx
                            .send_control(&encode_control(&Control::KeyframeRequest {
                                seq: frame.seq,
                            })?)
                            .await;
                        keyframe_requests += 1;
                    }
                    last_seq = Some(frame.seq);
                    video_decoder.push(&frame)?;
                    if let Some(view) = &controls.view {
                        // UI path: hand the newest image over (one frame only).
                        if let Some(image) = video_decoder.take_latest() {
                            *view.lock().expect("view slot") = Some(image);
                        }
                    } else if let Some(image) = video_decoder.take_latest() {
                        // CLI path: keep the decoder drained.
                        std::hint::black_box(image.rgba.len());
                    }
                }
                if last_metrics.elapsed() >= Duration::from_millis(500) {
                    last_metrics = Instant::now();
                    let elapsed_ms = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                    metrics.push(Sample {
                        elapsed_ms,
                        bitrate_bps: estimate_bitrate_bps(rx_bytes, elapsed_ms),
                        fps: params.fps,
                        rtt_ms: 0,
                        path: own_path,
                        dropped_frames: 0,
                    });
                    send(Event::Metrics(metrics.latest_line()));
                }
            }
        }
    }
    let _ = keyframe_requests;
    let report = finish_watch(
        metrics,
        reassembler,
        frames,
        rx_bytes,
        video_decoder.decoded_frames(),
        video_decoder.dropped_frames(),
        video_decoder.backend().unwrap_or("nenhum").to_string(),
        &controls,
    );
    video_decoder.stop();
    report
}

#[allow(clippy::too_many_arguments)]
fn finish_watch(
    metrics: Metrics,
    reassembler: Reassembler,
    frames: u64,
    rx_bytes: u64,
    decoded: u64,
    dropped_decode: u64,
    backend: String,
    controls: &Controls,
) -> Result<DemoReport> {
    Ok(DemoReport {
        frames,
        reassembled: reassembler.completed(),
        dropped_transport: reassembler.dropped(),
        dropped_pipeline: 0,
        decoded,
        dropped_decode,
        backend,
        tx_bytes: 0,
        rx_bytes,
        avg_bitrate_bps: metrics.avg_bitrate_bps(),
        metric_samples: metrics.len(),
        cancelled: controls.stop.is_cancelled(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoReport {
    pub frames: u64,
    pub reassembled: u64,
    pub dropped_transport: u64,
    pub dropped_pipeline: u64,
    /// Frames actually decoded by the viewer (0 when there is no decoder).
    pub decoded: u64,
    /// Frames the decoder refused/dropped (no reference, full queue).
    pub dropped_decode: u64,
    /// Encoder/decoder description, for honest status lines.
    pub backend: String,
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
            decoded: 0,
            dropped_decode: 0,
            backend: String::new(),
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
    fn invite_without_transport_fails_closed_for_remote_watch() {
        // Offline invites carry no rendezvous address; run_watch rejects them
        // before spawning anything (fail fast, no worker, no network).
        let token = Invite::create(1_700_000_000, DEFAULT_INVITE_TTL_SECS).encode();
        let parsed = Invite::decode(&token, 1_700_000_000).expect("convite válido");
        assert!(parsed.transport().is_none());
        assert!(!parsed.is_expired(1_700_000_000));
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
