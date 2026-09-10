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
use arti_client::{StreamPrefs, TorClient, config::TorClientConfigBuilder};
use directories::ProjectDirs;
use futures::future::BoxFuture;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    net::TcpListener,
    process::{Child, Command},
    time::timeout,
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
        .create_bootstrapped()
        .await?;
    let mut prefs = StreamPrefs::new();
    prefs.exit_country(exit_country.parse()?).ipv4_only();
    let connector = Arc::new(ArtiConnector(client.with_prefs(prefs)));
    let router = Router::new(connector.clone(), connector);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    println!("{}", listener.local_addr()?);
    std::io::stdout().flush()?;
    proxy::serve(listener, router, CancellationToken::new()).await
}

pub struct Worker {
    child: Child,
    pub address: SocketAddr,
}

impl Worker {
    pub async fn start(exit_country: &str) -> Result<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["tor-worker", "--country", exit_country])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let mut child = command
            .spawn()
            .context("não foi possível iniciar o cliente Tor")?;
        let stdout = child.stdout.take().context("worker sem stdout")?;
        let mut line = String::new();
        // Bound both bootstrap time and protocol output.
        timeout(
            Duration::from_secs(180),
            BufReader::new(stdout.take(128)).read_line(&mut line),
        )
        .await
        .context("o Tor não conectou em 180 segundos; tente novamente")??;
        let address: SocketAddr = line
            .trim()
            .parse()
            .context("o cliente Tor encerrou antes de ficar pronto")?;
        ensure!(
            address.ip().is_loopback() && address.port() != 0,
            "endereço inválido do cliente Tor"
        );
        Ok(Self { child, address })
    }

    pub async fn stop(&mut self) -> Result<()> {
        drop(self.child.stdin.take());
        match timeout(Duration::from_secs(3), self.child.wait()).await {
            Ok(result) => {
                result?;
            }
            Err(_) => self.child.kill().await?,
        }
        Ok(())
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
