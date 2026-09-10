use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
    sync::{Arc, mpsc::Sender},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use tokio::{
    net::TcpListener,
    time::{interval, timeout},
};
use tokio_util::sync::CancellationToken;

use crate::{
    discord::{Installation, Monitor},
    proxy::{self, Direct, Router, SocksUpstream},
    tor::{self, Worker},
};

#[derive(Debug, Clone)]
pub struct Options {
    pub country: String,
    pub warmup_secs: u64,
    pub manual: bool,
    pub discord: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Status(String),
    Warming(Option<Instant>),
    Direct,
    Finished,
    Error(String),
}

#[derive(Clone, Default)]
pub struct Controls {
    pub direct: CancellationToken,
    pub stop: CancellationToken,
}

pub fn lock_instance() -> Result<File> {
    let dirs = tor::dirs()?;
    std::fs::create_dir_all(dirs.data_local_dir())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dirs.data_local_dir().join("session.lock"))?;
    lock.try_lock_exclusive()
        .context("Já existe uma sessão ou teste de conexão do RelayHop em andamento. Aguarde o teste terminar ou use a janela existente.")?;
    Ok(lock)
}

pub async fn run(options: Options, controls: Controls, events: Sender<Event>) -> Result<()> {
    let _lock = lock_instance()?;
    let status = |message: &str| {
        let _ = events.send(Event::Status(message.into()));
    };
    let mut monitor = Monitor::new();
    ensure!(
        !monitor.running(),
        "Feche o Discord pelo ícone da bandeja e tente novamente. Uma instância já aberta ignora os novos parâmetros de conexão."
    );
    let installation = Installation::discover(options.discord.as_deref())?;
    status(&format!(
        "Conectando ao Tor (saída {})… A primeira conexão pode demorar.",
        options.country
    ));
    let mut worker = tokio::select! {
        _ = controls.stop.cancelled() => return Ok(()),
        result = Worker::start(&options.country) => result?,
    };
    status("Testando acesso HTTPS ao Discord pela rede Tor…");
    tokio::select! {
        _ = controls.stop.cancelled() => return Ok(()),
        result = tor::check_discord(worker.address) => result.context("Esta saída Tor não conseguiu acessar o Discord. Tente novamente ou selecione outro país.")?,
    }
    // The user may have opened Discord during the bootstrap.
    ensure!(
        !monitor.running(),
        "O Discord foi aberto enquanto o Tor conectava. Feche-o e tente novamente."
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let router = Router::new(Arc::new(SocksUpstream(worker.address)), Arc::new(Direct));
    let proxy_stop = CancellationToken::new();
    let mut proxy_task = tokio::spawn(proxy::serve(listener, router.clone(), proxy_stop.clone()));
    status(&format!("Abrindo {}…", installation.label));
    // Always clean up the server, including when launch fails.
    let result: Result<()> = async {
        let mut child = installation.launch(address)?;
        let mut poll = interval(Duration::from_secs(1));
        let startup = Instant::now();
        let mut seen_discord = false;
        let mut absent_ticks = 0;
        let mut deadline = None;
        let mut first_connection = Box::pin(router.first_connection.notified());
        let mut received_connection = false;
        let mut direct = false;
        let _ = events.send(Event::Status("Aguardando a primeira conexão do Discord pelo RelayHop…".into()));
        loop {
            tokio::select! {
                biased;
                _ = controls.stop.cancelled() => break,
                result = &mut proxy_task => {
                    result??;
                    bail!("o encaminhador local encerrou inesperadamente");
                }
                _ = controls.direct.cancelled(), if received_connection && !direct => {
                    router.switch_direct();
                    worker.stop().await?;
                    direct = true;
                    let _ = events.send(Event::Direct);
                }
                _ = &mut first_connection, if !received_connection => {
                    received_connection = true;
                    deadline = (!options.manual).then(|| Instant::now() + Duration::from_secs(options.warmup_secs));
                    let _ = events.send(Event::Warming(deadline));
                }
                _ = poll.tick() => {
                    if monitor.running() {
                        seen_discord = true;
                        absent_ticks = 0;
                    } else if seen_discord {
                        absent_ticks += 1;
                        if absent_ticks >= 3 { break; }
                    }
                    if !seen_discord && startup.elapsed() > Duration::from_secs(45) {
                        bail!("O processo do Discord não foi encontrado após a abertura. Verifique --discord.");
                    }
                    if !received_connection && startup.elapsed() > Duration::from_secs(90) {
                        bail!("O Discord não usou o proxy em 90 segundos. Esta instalação pode estar ignorando as flags; feche o Discord e verifique o launcher.");
                    }
                    if let Some(exit) = child.try_wait()? && !exit.success() && !seen_discord {
                        bail!("O launcher do Discord encerrou com {exit}");
                    }
                    if !direct {
                        ensure!(!worker.exited()?, "O cliente Tor encerrou antes da troca para conexão direta.");
                        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                            router.switch_direct();
                            worker.stop().await?;
                            direct = true;
                            let _ = events.send(Event::Direct);
                        }
                    }
                }
            }
        }
        Ok(())
    }.await;
    proxy_stop.cancel();
    if !proxy_task.is_finished() {
        // Abort if the bounded task shutdown unexpectedly stalls.
        if timeout(Duration::from_secs(2), &mut proxy_task)
            .await
            .is_err()
        {
            proxy_task.abort();
        }
    }
    worker.stop().await?;
    result
}
