//! Arti lives in a subprocess of this very executable so stopping it releases
//! all Tor tasks, sockets and memory. EOF on the parent pipe prevents orphans.
use std::{
    io::{Read, Write},
    net::SocketAddr,
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use arti_client::{
    StreamPrefs, TorClient,
    config::TorClientConfigBuilder,
    status::{BlockageKind, BootstrapStatus},
};
use directories::ProjectDirs;
use futures::{StreamExt, future::BoxFuture};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader},
    net::TcpListener,
    process::{Child, Command},
    time::{sleep, timeout},
};
use tokio_util::{compat::FuturesAsyncReadCompatExt, sync::CancellationToken};
use tor_rtcompat::PreferredRuntime;

use crate::proxy::{self, Connector, Router, Stream, Target};

pub fn dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "relayhop")
        .context("não foi possível localizar os diretórios do usuário")
}

pub fn country(value: &str) -> Result<String, String> {
    let value = value.to_ascii_uppercase();
    if value.len() != 2 || !value.bytes().all(|c| c.is_ascii_alphabetic()) || value == "BR" {
        return Err("use um código de país de duas letras fora do Brasil, como US ou DE".into());
    }
    Ok(value)
}

struct ArtiConnector(Arc<TorClient<PreferredRuntime>>);
impl Connector for ArtiConnector {
    fn connect<'a>(&'a self, target: &'a Target) -> BoxFuture<'a, Result<Stream>> {
        Box::pin(async move {
            let stream = self.0.connect((target.host.as_str(), target.port)).await?;
            Ok(Box::new(stream.compat()) as Stream)
        })
    }
}

const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_PROTOCOL_LINE: usize = 4_096;

fn clean_protocol_payload(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(512)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn emit_protocol(kind: &str, payload: &str) -> Result<()> {
    println!("{kind}\t{}", clean_protocol_payload(payload));
    std::io::stdout().flush()?;
    Ok(())
}

fn bootstrap_status(status: &BootstrapStatus) -> String {
    let percent = (status.as_frac() * 100.0).round() as u32;
    let detail = status.blocked().map(|blockage| match blockage.kind() {
        BlockageKind::Disabled => "inicialização ainda não foi liberada",
        BlockageKind::Offline => "sem conexão TCP; verifique a internet, o firewall ou o antivírus",
        BlockageKind::Filtering => "a conexão parece filtrada pelo firewall ou pela rede",
        BlockageKind::CantReachTor => "a rede Tor parece inacessível nesta conexão",
        BlockageKind::ClockSkewed => "relógio do sistema parece incorreto",
        BlockageKind::CantBootstrap => "não foi possível baixar o diretório Tor",
        _ => "a conexão não está avançando",
    });
    match detail {
        Some(detail) => format!("Conectando ao Tor… {percent}% — {detail}."),
        None => format!("Conectando ao Tor… {percent}%"),
    }
}

pub async fn worker(exit_country: &str) -> Result<()> {
    // A dedicated blocking OS thread can exit even if the async runtime is stuck.
    // The parent holds stdin open for the worker's entire lifetime.
    std::thread::spawn(|| {
        let mut byte = [0; 1];
        while matches!(std::io::stdin().read(&mut byte), Ok(1)) {}
        std::process::exit(0);
    });
    let paths = dirs()?;
    let config = TorClientConfigBuilder::from_directories(
        paths.data_local_dir().join("tor-state"),
        paths.cache_dir().join("tor"),
    );
    let client = TorClient::with_runtime(PreferredRuntime::current()?)
        .config(config.build()?)
        .create_unbootstrapped_async()
        .await?;
    let mut events = client.bootstrap_events();
    let initial = bootstrap_status(&client.bootstrap_status());
    emit_protocol("PROGRESS", &initial)?;
    let mut last_status = initial;
    let bootstrap = client.bootstrap();
    tokio::pin!(bootstrap);
    loop {
        tokio::select! {
            result = &mut bootstrap => {
                result?;
                break;
            }
            Some(event) = events.next() => {
                let current = bootstrap_status(&event);
                if current != last_status {
                    emit_protocol("PROGRESS", &current)?;
                    last_status = current;
                }
            }
        }
    }
    let mut prefs = StreamPrefs::new();
    prefs.exit_country(exit_country.parse()?).ipv4_only();
    let connector = Arc::new(ArtiConnector(client.with_prefs(prefs)));
    let router = Router::new(connector.clone(), connector);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    emit_protocol("READY", &listener.local_addr()?.to_string())?;
    proxy::serve(listener, router, CancellationToken::new()).await
}

pub async fn worker_entry(exit_country: &str) -> Result<()> {
    match worker(exit_country).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = emit_protocol("ERROR", &format!("{error:#}"));
            Err(error)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum WorkerMessage {
    Progress(String),
    Ready(SocketAddr),
    Error(String),
}

fn parse_worker_message(line: &str) -> Result<WorkerMessage> {
    let (kind, payload) = line
        .trim_end_matches(['\r', '\n'])
        .split_once('\t')
        .context("resposta inválida do cliente Tor")?;
    ensure!(!payload.is_empty(), "resposta vazia do cliente Tor");
    match kind {
        "PROGRESS" => Ok(WorkerMessage::Progress(payload.into())),
        "READY" => Ok(WorkerMessage::Ready(
            payload
                .parse()
                .context("endereço inválido do cliente Tor")?,
        )),
        "ERROR" => Ok(WorkerMessage::Error(payload.into())),
        _ => anyhow::bail!("resposta desconhecida do cliente Tor"),
    }
}

async fn read_protocol_line(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        ensure!(
            line.len() + consumed <= MAX_PROTOCOL_LINE,
            "resposta muito grande do cliente Tor"
        );
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    String::from_utf8(line)
        .context("resposta inválida do cliente Tor")
        .map(Some)
}

async fn stop_child(child: &mut Child) -> Result<()> {
    drop(child.stdin.take());
    if let Ok(result) = timeout(Duration::from_secs(3), child.wait()).await {
        result?;
        return Ok(());
    }
    if child.try_wait()?.is_none() {
        child
            .start_kill()
            .context("não foi possível encerrar o cliente Tor")?;
    }
    timeout(Duration::from_secs(2), child.wait())
        .await
        .context("o cliente Tor não respondeu ao encerramento")??;
    Ok(())
}

pub struct Worker {
    child: Child,
    pub address: SocketAddr,
}

impl Worker {
    pub async fn start(
        exit_country: &str,
        stop: &CancellationToken,
        mut on_status: impl FnMut(String),
    ) -> Result<Option<Self>> {
        if stop.is_cancelled() {
            return Ok(None);
        }
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["tor-worker", "--country", exit_country])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        let diagnostic_logs = std::env::var_os("RUST_LOG").is_some();
        #[cfg(windows)]
        {
            // Explorer launches have no useful stderr handle. For an explicit
            // diagnostic session, pipe logs back through the console-owning
            // parent instead of asking the GUI worker to inherit that handle.
            if diagnostic_logs {
                command.stderr(Stdio::piped());
            } else {
                command.stderr(Stdio::null());
            }
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        #[cfg(not(windows))]
        command.stderr(Stdio::inherit());
        let mut child = command
            .spawn()
            .context("não foi possível iniciar o cliente Tor")?;
        #[cfg(windows)]
        if diagnostic_logs && let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    eprintln!("[Tor] {line}");
                }
            });
        }
        let stdout = child.stdout.take().context("worker sem stdout")?;
        let mut stdout = BufReader::new(stdout);
        let deadline = sleep(BOOTSTRAP_TIMEOUT);
        tokio::pin!(deadline);
        let mut last_status = None;
        loop {
            let line = tokio::select! {
                _ = stop.cancelled() => {
                    stop_child(&mut child).await?;
                    return Ok(None);
                }
                _ = &mut deadline => {
                    let _ = stop_child(&mut child).await;
                    let detail = last_status
                        .as_deref()
                        .map(|status| format!(" Último estado: {status}"))
                        .unwrap_or_default();
                    anyhow::bail!(
                        "o Tor não conectou em {} segundos.{detail}",
                        BOOTSTRAP_TIMEOUT.as_secs()
                    );
                }
                result = read_protocol_line(&mut stdout) => result?,
            };
            let Some(line) = line else {
                let exit = child.wait().await?;
                anyhow::bail!("o cliente Tor encerrou antes de ficar pronto ({exit})");
            };
            match parse_worker_message(&line)? {
                WorkerMessage::Progress(status) => {
                    last_status = Some(status.clone());
                    on_status(status);
                }
                WorkerMessage::Ready(address) => {
                    ensure!(
                        address.ip().is_loopback() && address.port() != 0,
                        "endereço inválido do cliente Tor"
                    );
                    return Ok(Some(Self { child, address }));
                }
                WorkerMessage::Error(error) => {
                    let _ = stop_child(&mut child).await;
                    anyhow::bail!("cliente Tor: {error}");
                }
            }
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        stop_child(&mut self.child).await
    }

    pub fn exited(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }
}

pub async fn check_discord(address: SocketAddr) -> Result<()> {
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("socks5h://{address}"))?)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(45))
        .build()?;
    let mut response = client
        .get("https://discord.com/api/v10/gateway")
        .send()
        .await?
        .error_for_status()
        .context("o Discord recusou esta saída Tor")?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= 16_384,
            "resposta de teste muito grande"
        );
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value = serde_json::from_slice(&body)?;
    let url = reqwest::Url::parse(
        value["url"]
            .as_str()
            .context("resposta inesperada do Discord")?,
    )?;
    ensure!(
        url.scheme() == "wss" && url.host_str() == Some("gateway.discord.gg"),
        "resposta inesperada do Discord"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_protocol_accepts_only_expected_bounded_messages() -> Result<()> {
        assert_eq!(
            clean_protocol_payload("erro\r\nforjado\t oculto"),
            "erro forjado oculto"
        );
        assert_eq!(clean_protocol_payload(&"x".repeat(600)).len(), 512);
        assert_eq!(
            parse_worker_message("PROGRESS\tConectando… 15%\n")?,
            WorkerMessage::Progress("Conectando… 15%".into())
        );
        assert_eq!(
            parse_worker_message("READY\t127.0.0.1:4242\r\n")?,
            WorkerMessage::Ready("127.0.0.1:4242".parse()?)
        );
        assert!(parse_worker_message("READY\tnot-an-address\n").is_err());
        assert!(parse_worker_message("UNKNOWN\tvalue\n").is_err());
        assert!(parse_worker_message("PROGRESS\t\n").is_err());
        Ok(())
    }

    #[tokio::test]
    async fn protocol_reader_rejects_oversized_lines() {
        let input = vec![b'x'; MAX_PROTOCOL_LINE + 1];
        let mut reader = BufReader::new(input.as_slice());
        assert!(read_protocol_line(&mut reader).await.is_err());
    }
}
