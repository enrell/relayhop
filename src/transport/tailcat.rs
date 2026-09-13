//! Supervised Tailcat worker subprocess (`worker/tailcat/relay`).
//!
//! The worker carries exactly one control stream (TCP) and media datagrams
//! (UDP) for this app only. Framing matches the worker's stdio protocol;
//! every decode is bounded before allocating. EOF on our stdin handle (via
//! dropping it) terminates the worker; `kill_on_drop` is a backstop.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::mpsc,
    time::{Duration, timeout},
};
use tokio_util::sync::CancellationToken;

use crate::metrics::PathKind;

/// Frame types parent -> worker (must match worker/tailcat/relay).
pub const F_TO_PEER_TCP: u8 = 0x01;
pub const F_TO_PEER_UDP: u8 = 0x02;
pub const F_CLOSE: u8 = 0xFF;
/// Frame types worker -> parent.
pub const F_FROM_PEER_TCP: u8 = 0x81;
pub const F_FROM_PEER_UDP: u8 = 0x82;

pub const MAX_FRAME_BYTES: usize = 1 << 20;
pub const MAX_UDP_BYTES: usize = crate::protocol::MAX_UDP_DATAGRAM;
pub const MAX_EVENT_LINE: usize = 16_384;
pub const READY_TIMEOUT_SECS: u64 = 120;
pub const PEER_TIMEOUT_SECS: u64 = 180;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerEvent {
    Ready { addr: String },
    Peer,
    Path(PathKind),
    Closed { reason: String },
    Error(String),
}

fn parse_event_line(line: &str) -> Result<Option<WorkerEvent>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    ensure!(
        line.len() <= MAX_EVENT_LINE,
        "linha de evento grande demais"
    );
    let value: serde_json::Value = serde_json::from_str(line)?;
    let kind = value["t"].as_str().unwrap_or("");
    let event = match kind {
        "ready" => {
            let addr = value["addr"].as_str().context("ready sem addr")?;
            ensure!(!addr.is_empty() && addr.len() <= 1024, "addr inválido");
            WorkerEvent::Ready { addr: addr.into() }
        }
        "peer" => WorkerEvent::Peer,
        "path" => {
            let kind = value["kind"].as_str().unwrap_or("unknown");
            WorkerEvent::Path(match kind {
                "direct" => match value["endpoint"].as_str().unwrap_or("") {
                    ep if ep.contains(":[") || ep.matches(':').count() > 1 => PathKind::DirectIpv6,
                    ep if !ep.is_empty() => PathKind::DirectIpv4,
                    _ => PathKind::Unknown,
                },
                "relay" => PathKind::Relay,
                _ => PathKind::Unknown,
            })
        }
        "closed" => WorkerEvent::Closed {
            reason: value["reason"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(256)
                .collect(),
        },
        "error" => WorkerEvent::Error(
            value["error"]
                .as_str()
                .unwrap_or("erro")
                .chars()
                .take(256)
                .collect(),
        ),
        _ => bail!("evento desconhecido do worker"),
    };
    Ok(Some(event))
}

/// Resolve the worker binary without a shell: explicit env override, then
/// next to the current executable, then PATH.
pub fn find_worker_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RELAYHOP_TAILCAT_BIN") {
        let path = PathBuf::from(path);
        ensure!(
            path.is_file(),
            "binário do worker não encontrado (RELAYHOP_TAILCAT_BIN={}): compile com go build em worker/tailcat/relay",
            path.display()
        );
        return Ok(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for name in ["relayhop-tailcat", "relayhop-tailcat.exe"] {
            let path = dir.join(name);
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("relayhop-tailcat"))
            .find(|path| path.is_file())
    }) {
        return Ok(path);
    }
    bail!(
        "worker Tailcat não encontrado. Compile com: cd worker/tailcat/relay && go build -o relayhop-tailcat .; \
         e aponte RELAYHOP_TAILCAT_BIN ou coloque ao lado do executável."
    )
}

pub struct WorkerTx {
    child: Child,
    stdin: Option<ChildStdin>,
}

pub struct WorkerRx {
    pub control_rx: mpsc::Receiver<Vec<u8>>,
    pub media_rx: mpsc::Receiver<Vec<u8>>,
    pub events: mpsc::Receiver<WorkerEvent>,
}

pub struct Worker {
    tx: WorkerTx,
    rx: WorkerRx,
}

impl Worker {
    /// Split into independently owned halves for `select!` loops.
    pub fn split(self) -> (WorkerTx, WorkerRx) {
        (self.tx, self.rx)
    }
}

impl Worker {
    async fn spawn_common(
        bin: &Path,
        mode: &str,
        extra: &[String],
        tcp_port: u16,
        udp_port: u16,
        derp_map: &str,
        stop: &CancellationToken,
    ) -> Result<Self> {
        let mut command = Command::new(bin);
        command
            .arg(mode)
            .arg("--tcp-port")
            .arg(tcp_port.to_string())
            .arg("--udp-port")
            .arg(udp_port.to_string())
            .args(extra)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if !derp_map.is_empty() {
            command.arg("--derpmap-url").arg(derp_map);
        }
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .context("não foi possível iniciar o worker Tailcat")?;
        let stdin = child.stdin.take().context("worker sem stdin")?;
        let stdout = child.stdout.take().context("worker sem stdout")?;
        let stderr = child.stderr.take().context("worker sem stderr")?;
        let (control_tx, control_rx) = mpsc::channel(64);
        let (media_tx, media_rx) = mpsc::channel(256);
        let (event_tx, events) = mpsc::channel(32);
        let stop_clone = stop.clone();
        tokio::spawn(stdout_pump(
            stdout,
            control_tx,
            media_tx,
            event_tx.clone(),
            stop_clone,
        ));
        let stop_clone = stop.clone();
        tokio::spawn(stderr_pump(stderr, event_tx, stop_clone));
        Ok(Self {
            tx: WorkerTx {
                child,
                stdin: Some(stdin),
            },
            rx: WorkerRx {
                control_rx,
                media_rx,
                events,
            },
        })
    }

    pub async fn serve(
        tcp_port: u16,
        udp_port: u16,
        derp_map: &str,
        stop: &CancellationToken,
    ) -> Result<Self> {
        let bin = find_worker_binary()?;
        Self::spawn_common(&bin, "serve", &[], tcp_port, udp_port, derp_map, stop).await
    }

    pub async fn join(
        addr: &str,
        tcp_port: u16,
        udp_port: u16,
        derp_map: &str,
        stop: &CancellationToken,
    ) -> Result<Self> {
        ensure!(!addr.is_empty() && addr.len() <= 1024, "endereço inválido");
        let bin = find_worker_binary()?;
        Self::spawn_common(
            &bin,
            "join",
            &[String::from("--addr"), addr.to_string()],
            tcp_port,
            udp_port,
            derp_map,
            stop,
        )
        .await
    }
}

impl WorkerTx {
    /// Send one control payload to the peer TCP stream.
    pub async fn send_control(&mut self, body: &[u8]) -> Result<()> {
        self.send_frame(F_TO_PEER_TCP, body).await
    }

    /// Send one media datagram to the peer (bounded to 1232 B).
    pub async fn send_media(&mut self, datagram: &[u8]) -> Result<()> {
        ensure!(
            datagram.len() <= MAX_UDP_BYTES,
            "datagrama excede 1232 bytes"
        );
        self.send_frame(F_TO_PEER_UDP, datagram).await
    }

    async fn send_frame(&mut self, typ: u8, body: &[u8]) -> Result<()> {
        ensure!(body.len() < MAX_FRAME_BYTES, "frame grande demais");
        let Some(stdin) = self.stdin.as_mut() else {
            bail!("worker encerrado")
        };
        let mut header = [0u8; 5];
        header[..4].copy_from_slice(&(body.len() as u32 + 1).to_be_bytes());
        header[4] = typ;
        timeout(Duration::from_secs(10), stdin.write_all(&header))
            .await?
            .context("worker não aceitou o frame")?;
        timeout(Duration::from_secs(10), stdin.write_all(body))
            .await?
            .context("worker não aceitou o frame")?;
        Ok(())
    }

    /// Orderly shutdown: ask the worker to close, drop stdin (EOF), wait.
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(mut stdin) = self.stdin.take() {
            let mut header = [0u8; 5];
            header[..4].copy_from_slice(&1u32.to_be_bytes());
            header[4] = F_CLOSE;
            let _ = timeout(Duration::from_secs(2), stdin.write_all(&header)).await;
            drop(stdin);
        }
        match timeout(Duration::from_secs(5), self.child.wait()).await {
            Ok(result) => {
                result?;
            }
            Err(_) => {
                self.child.kill().await?;
            }
        }
        Ok(())
    }
}

async fn stdout_pump(
    mut stdout: ChildStdout,
    control_tx: mpsc::Sender<Vec<u8>>,
    media_tx: mpsc::Sender<Vec<u8>>,
    event_tx: mpsc::Sender<WorkerEvent>,
    stop: CancellationToken,
) {
    let mut header = [0u8; 5];
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            result = stdout.read_exact(&mut header) => {
                if result.is_err() { break; }
            }
        }
        let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let typ = header[4];
        if !(1..=MAX_FRAME_BYTES).contains(&len) {
            let _ = event_tx
                .send(WorkerEvent::Error("frame inválido do worker".into()))
                .await;
            break;
        }
        let mut body = vec![0u8; len - 1];
        let read = tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            result = stdout.read_exact(&mut body) => result,
        };
        if read.is_err() {
            break;
        }
        let sent = match typ {
            F_FROM_PEER_TCP => control_tx.try_send(body).is_ok(),
            F_FROM_PEER_UDP => {
                if body.len() > MAX_UDP_BYTES {
                    let _ = event_tx
                        .send(WorkerEvent::Error("datagrama grande demais".into()))
                        .await;
                    break;
                }
                media_tx.try_send(body).is_ok()
            }
            _ => {
                let _ = event_tx
                    .send(WorkerEvent::Error("tipo de frame desconhecido".into()))
                    .await;
                break;
            }
        };
        if !sent {
            // Bounded queues: a slow consumer drops (media) or fails (control).
            // Control backpressure surfaces as an error; media loss is normal.
            if typ == F_FROM_PEER_TCP {
                let _ = event_tx
                    .send(WorkerEvent::Error("consumidor de controle lento".into()))
                    .await;
                break;
            }
        }
    }
}

/// Parse worker stderr JSON lines into lifecycle events. Tailcat's own
/// diagnostics share the stream; non-JSON lines are ignored.
async fn stderr_pump(
    stderr: tokio::process::ChildStderr,
    event_tx: mpsc::Sender<WorkerEvent>,
    stop: CancellationToken,
) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    // Bounded total stderr consumption; the worker is quiet unless --verbose
    // (all event lines are small; see MAX_EVENT_LINE).
    let mut lines = BufReader::new(stderr.take(1 << 20)).lines();
    loop {
        let line = tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            result = lines.next_line() => result,
        };
        match line {
            Ok(Some(text)) => match parse_event_line(&text) {
                Ok(Some(event)) => {
                    let terminal =
                        matches!(event, WorkerEvent::Closed { .. } | WorkerEvent::Error(_));
                    if event_tx.send(event).await.is_err() || terminal {
                        break;
                    }
                }
                Ok(None) => {}
                Err(_) => {}
            },
            _ => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_lines_parse_and_reject() -> Result<()> {
        let ready = parse_event_line(r#"{"t":"ready","addr":"tcABC"}"#)?.expect("evento");
        assert_eq!(
            ready,
            WorkerEvent::Ready {
                addr: "tcABC".into()
            }
        );
        assert_eq!(
            parse_event_line(r#"{"t":"peer"}"#)?.expect("e"),
            WorkerEvent::Peer
        );
        assert_eq!(
            parse_event_line(r#"{"t":"path","kind":"relay"}"#)?.expect("e"),
            WorkerEvent::Path(PathKind::Relay)
        );
        assert_eq!(
            parse_event_line(r#"{"t":"path","kind":"direct","endpoint":"203.0.113.7:5678"}"#)?
                .expect("e"),
            WorkerEvent::Path(PathKind::DirectIpv4)
        );
        assert_eq!(
            parse_event_line(r#"{"t":"path","kind":"direct","endpoint":"[2001:db8::7]:5678"}"#)?
                .expect("e"),
            WorkerEvent::Path(PathKind::DirectIpv6)
        );
        assert_eq!(
            parse_event_line(r#"{"t":"path","kind":"direct"}"#)?.expect("e"),
            WorkerEvent::Path(PathKind::Unknown)
        );
        assert!(parse_event_line(r#"{"t":"nope"}"#).is_err());
        assert!(parse_event_line("")?.is_none());
        assert!(parse_event_line(&"x".repeat(MAX_EVENT_LINE + 1)).is_err());
        Ok(())
    }

    #[test]
    fn missing_binary_is_actionable() {
        unsafe { std::env::set_var("RELAYHOP_TAILCAT_BIN", "/nonexistent/relayhop-tailcat-xyz") };
        let error = find_worker_binary().unwrap_err();
        assert!(format!("{error:#}").contains("go build"));
        unsafe { std::env::remove_var("RELAYHOP_TAILCAT_BIN") };
    }
}
