//! App-only transport: loopback for local validation plus a supervised
//! external Tailcat binary for real P2P (gate pending, see PLAN.md).
//!
//! Nothing here changes system routes, DNS, or proxy settings. The Tailcat
//! path is per-application traffic only; global VPN mode is out of scope.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;

use crate::metrics::PathKind;

pub mod tailcat;

pub const MAX_UDP_DATAGRAM_BYTES: usize = crate::protocol::MAX_UDP_DATAGRAM;
pub const LOOPBACK_QUEUE: usize = 64;

#[derive(Debug)]
pub struct LoopbackEndpoint {
    tx: mpsc::Sender<Vec<u8>>,
    rx: mpsc::Receiver<Vec<u8>>,
    pub path: PathKind,
    tx_bytes: u64,
    rx_bytes: u64,
}

impl LoopbackEndpoint {
    pub async fn send(&mut self, bytes: &[u8]) -> Result<()> {
        anyhow::ensure!(
            bytes.len() <= MAX_UDP_DATAGRAM_BYTES,
            "datagrama grande demais"
        );
        self.tx
            .send(bytes.to_vec())
            .await
            .map_err(|_| anyhow::anyhow!("par encerrado"))?;
        self.tx_bytes += bytes.len() as u64;
        Ok(())
    }

    pub async fn recv(&mut self, stop: &CancellationToken) -> Option<Vec<u8>> {
        tokio::select! {
            biased;
            _ = stop.cancelled() => None,
            message = self.rx.recv() => message.inspect(|bytes| {
                self.rx_bytes += bytes.len() as u64;
            }),
        }
    }

    pub fn counters(&self) -> (u64, u64) {
        (self.tx_bytes, self.rx_bytes)
    }

    /// Non-blocking drain used by the paced demo loop.
    pub fn try_recv(&mut self) -> Option<Vec<u8>> {
        self.rx.try_recv().ok().inspect(|bytes| {
            self.rx_bytes += bytes.len() as u64;
        })
    }
}

/// Cross-connected in-memory pair. Used by tests, `diag` and local demo.
pub fn loopback_pair() -> (LoopbackEndpoint, LoopbackEndpoint) {
    let (a_tx, b_rx) = mpsc::channel(LOOPBACK_QUEUE);
    let (b_tx, a_rx) = mpsc::channel(LOOPBACK_QUEUE);
    (
        LoopbackEndpoint {
            tx: a_tx,
            rx: a_rx,
            path: PathKind::Local,
            tx_bytes: 0,
            rx_bytes: 0,
        },
        LoopbackEndpoint {
            tx: b_tx,
            rx: b_rx,
            path: PathKind::Local,
            tx_bytes: 0,
            rx_bytes: 0,
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailcatAvailability {
    Available { version: String },
    Missing,
    Incompatible { reason: String },
}

pub fn default_bin_name() -> &'static str {
    "tailcat"
}

/// Honest local candidate hints: stack presence only, never a claim about
/// real reachability. Public DERP/IPv6 reachability stays opt-in manual work.
pub fn candidate_hints() -> Vec<(PathKind, bool, &'static str)> {
    let v6 = std::net::UdpSocket::bind("[::1]:0").is_ok();
    let v4 = std::net::UdpSocket::bind("127.0.0.1:0").is_ok();
    let relay = find_on_path().is_some();
    vec![
        (PathKind::Local, true, "loopback validado offline"),
        (
            PathKind::DirectIpv6,
            v6,
            "stack IPv6 presente; alcance real não testado",
        ),
        (
            PathKind::DirectIpv4,
            v4,
            "stack IPv4 presente; alcance real não testado",
        ),
        (
            PathKind::Relay,
            relay,
            "binário tailcat no PATH (versão checada no diagnóstico)",
        ),
    ]
}

/// Search PATH without invoking a shell.
pub fn find_on_path() -> Option<PathBuf> {
    let name = default_bin_name();
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    })
}

fn first_line_trimmed(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("tailcat")
        .chars()
        .take(64)
        .collect()
}

/// Probe an explicit Tailcat binary with structured args (no shell).
/// The binary is optional: a missing binary is a normal `Missing` result.
pub async fn probe_tailcat(bin: &Path) -> TailcatAvailability {
    let mut command = tokio::process::Command::new(bin);
    command.arg("--help").kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let child = match command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return TailcatAvailability::Missing;
        }
        Err(error) => {
            return TailcatAvailability::Incompatible {
                reason: format!("não foi possível executar: {error}"),
            };
        }
    };
    let output = timeout(Duration::from_secs(5), child.wait_with_output()).await;
    match output {
        Ok(Ok(output)) if output.status.success() => TailcatAvailability::Available {
            version: first_line_trimmed(&String::from_utf8_lossy(
                &output.stdout[..8192.min(output.stdout.len())],
            )),
        },
        Ok(Ok(output)) => TailcatAvailability::Incompatible {
            reason: format!("saída com status {}", output.status),
        },
        Ok(Err(error)) => TailcatAvailability::Incompatible {
            reason: format!("falha de E/S: {error}"),
        },
        Err(_) => TailcatAvailability::Incompatible {
            reason: "tempo esgotado ao sondar o binário".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loopback_moves_bounded_datagrams() -> Result<()> {
        let (mut a, mut b) = loopback_pair();
        let stop = CancellationToken::new();
        a.send(&[1, 2, 3]).await?;
        let got = b.recv(&stop).await.expect("datagrama");
        assert_eq!(got, vec![1, 2, 3]);
        assert!(
            a.send(&vec![0u8; MAX_UDP_DATAGRAM_BYTES + 1])
                .await
                .is_err()
        );
        let (tx_bytes, _) = a.counters();
        assert_eq!(tx_bytes, 3);
        Ok(())
    }

    #[test]
    fn help_first_line_is_bounded() {
        assert_eq!(
            first_line_trimmed("\n  tailcat v0.1.0\nmore"),
            "tailcat v0.1.0"
        );
    }

    #[tokio::test]
    async fn missing_binary_is_not_an_error() {
        let availability = probe_tailcat(Path::new("/nonexistent/relayhop-tailcat-probe")).await;
        assert!(matches!(availability, TailcatAvailability::Missing));
    }
}
