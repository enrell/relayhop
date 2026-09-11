use std::{
    fs::{File, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use eframe::egui;
use fs2::FileExt;

use crate::{tor, tray::Action};

const POLL_INTERVAL: Duration = Duration::from_millis(200);

pub enum Claim {
    Primary(Primary),
    Secondary,
}

pub struct Primary {
    _lock: File,
    request: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Primary {
    pub fn listen(&self, tx: Sender<Action>, context: egui::Context) {
        let request = self.request.clone();
        let stop = self.stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match take_request(&request) {
                    Ok(true) => {
                        let _ = tx.send(Action::Start);
                        #[cfg(windows)]
                        crate::tray::show_main_window();
                        context.request_repaint();
                    }
                    Ok(false) => {}
                    Err(error) => tracing::debug!(%error, "falha ao ler ativação secundária"),
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        });
    }
}

impl Drop for Primary {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub fn claim_or_signal(activate_existing: bool) -> Result<Claim> {
    let dirs = tor::dirs()?;
    claim_or_signal_in(dirs.data_local_dir(), activate_existing)
}

fn claim_or_signal_in(directory: &Path, activate_existing: bool) -> Result<Claim> {
    std::fs::create_dir_all(directory)?;
    let lock_path = directory.join("app.lock");
    let request = directory.join("activate.request");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;

    match lock.try_lock_exclusive() {
        Ok(()) => {
            let _ = std::fs::remove_file(&request);
            Ok(Claim::Primary(Primary {
                _lock: lock,
                request,
                stop: Arc::new(AtomicBool::new(false)),
            }))
        }
        Err(error) if lock_is_contended(&error) => {
            if activate_existing {
                std::fs::write(&request, std::process::id().to_string())
                    .context("não foi possível ativar a janela existente do RelayHop")?;
            }
            Ok(Claim::Secondary)
        }
        Err(error) => Err(error).context("não foi possível verificar a instância do RelayHop"),
    }
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

fn take_request(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_signals_the_primary() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let first = claim_or_signal_in(temp.path(), true)?;
        assert!(matches!(first, Claim::Primary(_)));

        let second = claim_or_signal_in(temp.path(), true)?;
        assert!(matches!(second, Claim::Secondary));
        assert!(take_request(&temp.path().join("activate.request"))?);
        assert!(!take_request(&temp.path().join("activate.request"))?);
        Ok(())
    }

    #[test]
    fn background_instance_does_not_activate_existing_window() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let first = claim_or_signal_in(temp.path(), true)?;
        assert!(matches!(first, Claim::Primary(_)));

        let second = claim_or_signal_in(temp.path(), false)?;
        assert!(matches!(second, Claim::Secondary));
        assert!(!take_request(&temp.path().join("activate.request"))?);
        Ok(())
    }
}
