#![cfg_attr(windows, windows_subsystem = "windows")]

mod display;
mod icon;
mod invite;
mod media;
mod metrics;
mod protocol;
mod session;
mod transport;
mod tray;
mod ui;

use std::sync::mpsc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "RelayHop — compartilhamento de tela P2P")]
struct Cli {
    /// Iniciar sem interface gráfica; mantenha o terminal aberto.
    #[arg(long)]
    headless: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Criar um convite e transmitir ao espectador via P2P.
    Host {
        /// Fonte de captura (somente sintética até o gate de mídia).
        #[arg(long, default_value = "synthetic-720p30")]
        source: String,
        /// Mapa DERP alternativo (vazio = padrão do worker).
        #[arg(long, default_value = "")]
        derp_map: String,
    },
    /// Assistir com um convite `rh1.…` recebido do transmissor.
    Watch {
        /// Convite `rh1.…` recebido do transmissor.
        invite: String,
        /// Mapa DERP alternativo (vazio = padrão do worker).
        #[arg(long, default_value = "")]
        derp_map: String,
    },
    /// Demonstração local ponta a ponta, sem rede pública.
    LocalDemo {
        /// Duração em segundos (1..=60).
        #[arg(long, default_value_t = 10)]
        secs: u64,
    },
    /// Diagnóstico local: protocolo, convite, loopback e binário Tailcat.
    Diag,
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
/// The stripped variables are saved for screen capture (see
/// display::preserve_display_env) so portals/PipeWire keep the session env.
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
    crate::display::preserve_display_env(&WAYLAND_VARS);
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
    unsafe {
        // Preserve CLI output when launched from a terminal; Explorer launches
        // show only the GUI.
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
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
        Some(Command::Host { source, derp_map }) => {
            runtime()?.block_on(host_command(source, derp_map))
        }
        Some(Command::Watch { invite, derp_map }) => {
            runtime()?.block_on(watch_command(invite, derp_map))
        }
        Some(Command::LocalDemo { secs }) => runtime()?.block_on(local_demo_command(secs)),
        Some(Command::Diag) => runtime()?.block_on(diag_command()),
        None => {
            if cli.headless {
                runtime()?.block_on(local_demo_command(10))
            } else {
                ui::run()
            }
        }
    }
}

async fn host_command(source: String, derp_map: String) -> Result<()> {
    let options = session::Options {
        source_id: source,
        derp_map,
        ..session::Options::default()
    };
    options.validate()?;
    let controls = session::Controls::default();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for event in rx {
            match event {
                session::Event::Status(message)
                | session::Event::Participant(message)
                | session::Event::Metrics(message) => eprintln!("{message}"),
                session::Event::InviteReady { redacted } => {
                    eprintln!("Sala: {redacted}");
                }
                // The full token is printed ONCE to stdout for copying.
                // It never appears in logs, status lines or errors.
                session::Event::InviteToken(token) => {
                    println!("Convite (compartilhe com o espectador): {token}");
                }
                session::Event::Finished => eprintln!("Sessão encerrada."),
                session::Event::Error(message) => eprintln!("Erro: {message}"),
            }
        }
    });
    let stop = controls.stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            stop.cancel();
        }
    });
    let report = session::run_host(options, controls, tx).await?;
    eprintln!(
        "Resumo: {} frames enviados, {} bytes, {} bps médios.",
        report.frames, report.tx_bytes, report.avg_bitrate_bps
    );
    Ok(())
}

async fn watch_command(invite: String, derp_map: String) -> Result<()> {
    let options = session::Options {
        derp_map,
        ..session::Options::default()
    };
    options.validate()?;
    let controls = session::Controls::default();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for event in rx {
            match event {
                session::Event::Status(message)
                | session::Event::Participant(message)
                | session::Event::Metrics(message) => eprintln!("{message}"),
                session::Event::InviteReady { .. } | session::Event::InviteToken(_) => {}
                session::Event::Finished => eprintln!("Sessão encerrada."),
                session::Event::Error(message) => eprintln!("Erro: {message}"),
            }
        }
    });
    let stop = controls.stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            stop.cancel();
        }
    });
    let report = session::run_watch(&invite, options, controls, tx).await?;
    eprintln!(
        "Resumo: {} frames remontados, {} bytes recebidos, {} bps médios.",
        report.reassembled, report.rx_bytes, report.avg_bitrate_bps
    );
    Ok(())
}

async fn local_demo_command(secs: u64) -> Result<()> {
    let options = session::Options {
        demo_secs: secs.clamp(1, 60),
        ..session::Options::default()
    };
    let controls = session::Controls::default();
    let (tx, rx) = mpsc::channel();
    let stop = controls.stop.clone();
    std::thread::spawn(move || {
        for event in rx {
            match event {
                session::Event::Status(message)
                | session::Event::Participant(message)
                | session::Event::Metrics(message) => eprintln!("{message}"),
                session::Event::InviteReady { redacted } => {
                    eprintln!("Sala local: {redacted}");
                }
                // Local demo never emits tokens; arm kept for exhaustiveness.
                session::Event::InviteToken(_) => {}
                session::Event::Finished => eprintln!("Demonstração concluída."),
                session::Event::Error(message) => eprintln!("Erro: {message}"),
            }
        }
    });
    let report_stop = stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            report_stop.cancel();
        }
    });
    let report = session::run_local_demo(options, controls, tx).await?;
    stop.cancel();
    eprintln!(
        "Resumo: {} frames ({} remontados), {} enviados/{} recebidos, {} descartes no pipeline/{} no transporte, {} bps médios em {} amostras{}.",
        report.frames,
        report.reassembled,
        report.tx_bytes,
        report.rx_bytes,
        report.dropped_pipeline,
        report.dropped_transport,
        report.avg_bitrate_bps,
        report.metric_samples,
        if report.cancelled { " (cancelado)" } else { "" }
    );
    Ok(())
}

async fn diag_command() -> Result<()> {
    println!("{}", session::diagnose().await?);
    Ok(())
}
