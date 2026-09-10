#![cfg_attr(windows, windows_subsystem = "windows")]

mod discord;
mod icon;
mod proxy;
mod session;
mod tor;
mod tray;
mod ui;

use std::{path::PathBuf, sync::mpsc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    version,
    about = "RelayHop — um salto temporário pelo Tor para abrir o Discord"
)]
struct Cli {
    /// Iniciar sem interface gráfica; mantenha o terminal aberto.
    #[arg(long)]
    headless: bool,
    /// País de saída Tor (GeoIP); não pode ser BR.
    #[arg(long, default_value = "US", value_parser = tor::country)]
    country: String,
    /// Segundos até voltar à conexão direta, contados da primeira conexão.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(10..=600))]
    warmup_secs: u64,
    /// Só trocar para conexão direta ao clicar no botão (ou ENTER no terminal).
    #[arg(long)]
    manual: bool,
    /// Caminho explícito do executável ou launcher do Discord.
    #[arg(long)]
    discord: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Internal>,
}

#[derive(Subcommand)]
enum Internal {
    #[command(hide = true)]
    TorWorker {
        #[arg(long, value_parser = tor::country)]
        country: String,
    },
    /// Testar Tor → conexão direta com HTTPS, sem abrir o Discord.
    Check,
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("não foi possível iniciar o runtime")
}

/// On Linux, prefer X11/XWayland when available: winit 0.30 picks Wayland
/// whenever WAYLAND_DISPLAY/WAYLAND_SOCKET is set, but it cannot hide
/// (Visible(false)) or un-minimize windows on Wayland, and Hyprland also
/// ignores minimize requests — so hide-to-tray cannot work there.
/// XWayland honors both, making Super+W/fechar actually collapse to the tray.
/// The stripped variables are saved for the Discord child (see
/// discord::preserve_display_env) so Discord keeps its validated environment.
/// Set RELAYHOP_WAYLAND=1 to keep native Wayland.
#[cfg(target_os = "linux")]
fn prefer_x11_backend() {
    if std::env::var_os("RELAYHOP_WAYLAND").is_some() {
        return;
    }
    const WAYLAND_VARS: [&str; 2] = ["WAYLAND_DISPLAY", "WAYLAND_SOCKET"];
    let wayland_present = WAYLAND_VARS
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()));
    if !wayland_present {
        return;
    }
    let display_set = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
    let socket_present = std::fs::read_dir("/tmp/.X11-unix").is_ok_and(|mut entries| {
        entries.any(|entry| entry.is_ok_and(|e| e.file_name().to_string_lossy().starts_with('X')))
    });
    if !(display_set && socket_present) {
        tracing::debug!(
            "XWayland indisponível; a bandeja funciona, mas recolher pode não ocultar a janela"
        );
        return;
    }
    crate::discord::preserve_display_env(&WAYLAND_VARS);
    // SAFETY: runs at startup before any thread spawns.
    for key in WAYLAND_VARS {
        unsafe { std::env::remove_var(key) };
    }
    tracing::debug!("interface via XWayland para permitir recolher à bandeja");
}

#[cfg(not(target_os = "linux"))]
fn prefer_x11_backend() {}

fn main() -> Result<()> {
    prefer_x11_backend();
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() != Some("tor-worker") {
        // Preserve CLI output when launched from a terminal; Explorer launches
        // show only the GUI. Never replace the worker's inherited control pipes.
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(
                windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "error".into()),
        )
        .with_target(false)
        .init();
    let cli = Cli::parse();
    match cli.command {
        Some(Internal::TorWorker { country }) => return runtime()?.block_on(tor::worker(&country)),
        Some(Internal::Check) => return runtime()?.block_on(connectivity_check(&cli.country)),
        None => {}
    }
    let options = session::Options {
        country: cli.country,
        warmup_secs: cli.warmup_secs,
        manual: cli.manual,
        discord: cli.discord,
    };
    if cli.headless {
        headless(options)
    } else {
        ui::run(options)
    }
}

async fn connectivity_check(country: &str) -> Result<()> {
    let _lock = session::lock_instance()?;
    eprintln!("Conectando ao Tor com saída {country}…");
    let mut worker = tor::Worker::start(country).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let router = proxy::Router::new(
        std::sync::Arc::new(proxy::SocksUpstream(worker.address)),
        std::sync::Arc::new(proxy::Direct),
    );
    let stop = tokio_util::sync::CancellationToken::new();
    let server = tokio::spawn(proxy::serve(listener, router.clone(), stop.clone()));
    let result: Result<()> = async {
        tor::check_discord(address)
            .await
            .context("teste da rota Tor")?;
        eprintln!("HTTPS pelo Tor: OK. Trocando para conexão direta…");
        router.switch_direct();
        worker.stop().await?;
        tor::check_discord(address)
            .await
            .context("teste da rota direta")?;
        eprintln!("HTTPS direto: OK. Tor encerrado. Isso não verifica a liberação de streaming.");
        Ok(())
    }
    .await;
    stop.cancel();
    server.await??;
    worker.stop().await?;
    result
}

fn headless(options: session::Options) -> Result<()> {
    let controls = session::Controls::default();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for event in rx {
            match event {
                session::Event::Status(message) => eprintln!("{message}"),
                session::Event::Warming(deadline) => eprintln!(
                    "Discord conectado pelo Tor. {} ENTER troca para conexão direta.",
                    if deadline.is_some() {
                        "Temporizador iniciado."
                    } else {
                        "Modo manual."
                    }
                ),
                session::Event::Direct => eprintln!(
                    "Conexão direta. Tor encerrado. Mantenha o RelayHop aberto enquanto usa o Discord."
                ),
                _ => {}
            }
        }
    });
    let direct = controls.direct.clone();
    std::thread::spawn(move || {
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_ok_and(|n| n > 0) {
            direct.cancel();
        }
    });
    let stop = controls.stop.clone();
    runtime()?.block_on(async move {
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stop.cancel();
            }
        });
        session::run(options, controls, tx).await
    })
}
