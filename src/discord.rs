use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
};

#[cfg(target_os = "linux")]
use std::sync::OnceLock;

use anyhow::{Context, Result, bail, ensure};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::process::{Child, Command};

/// Display variables saved before the GUI backend selection strips them
/// (see main::prefer_x11_backend). The Discord child gets the exact
/// environment the user validated, regardless of which backend our own
/// window uses. Linux-only: only Linux strips these variables.
#[cfg(target_os = "linux")]
static PRESERVED_DISPLAY_ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();

#[cfg(target_os = "linux")]
pub fn preserve_display_env(keys: &[&str]) {
    let _ = PRESERVED_DISPLAY_ENV.set(
        keys.iter()
            .filter_map(|key| {
                std::env::var(key)
                    .ok()
                    .map(|value| ((*key).to_string(), value))
            })
            .collect(),
    );
}

#[derive(Debug, Clone)]
pub struct Installation {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub label: String,
}

impl Installation {
    pub fn discover(explicit: Option<&Path>) -> Result<Self> {
        if let Some(path) = explicit {
            ensure!(
                path.is_file(),
                "executável não encontrado: {}",
                path.display()
            );
            return Ok(Self::native(path.to_path_buf()));
        }
        discover_platform().context("Discord não encontrado. Use --discord /caminho/do/executável ou informe o caminho na interface.")
    }

    fn native(program: PathBuf) -> Self {
        Self {
            label: program.display().to_string(),
            program,
            arguments: vec![],
        }
    }

    pub fn launch(&self, proxy: std::net::SocketAddr) -> Result<Child> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.arguments)
            .args(proxy_arguments(proxy))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Restore the display environment validated by the user; our own
        // process may have stripped it for the winit backend selection.
        #[cfg(target_os = "linux")]
        if let Some(vars) = PRESERVED_DISPLAY_ENV.get() {
            for (key, value) in vars {
                command.env(key, value);
            }
        }
        if self.arguments.is_empty()
            && let Some(parent) = self.program.parent()
            && !parent.as_os_str().is_empty()
        {
            command.current_dir(parent);
        }
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        command
            .spawn()
            .with_context(|| format!("não foi possível abrir {}", self.label))
    }
}

pub fn proxy_arguments(proxy: std::net::SocketAddr) -> Vec<String> {
    vec![
        format!("--proxy-server=socks5://{proxy}"),
        // Native voice UDP isn't handled by Chromium's SOCKS proxy. These
        // media/CDN hostnames also bypass it for supported TCP requests.
        "--proxy-bypass-list=discord.media;*.discord.media;cdn.discordapp.com;discordapp.net;*.discordapp.net".into(),
    ]
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

#[cfg(target_os = "linux")]
fn discover_platform() -> Result<Installation> {
    if let Some(path) = on_path("discord").or_else(|| on_path("Discord")) {
        // Some distros expose Snap through a symlink named "discord".
        return Ok(Installation::native(path));
    }
    if let Some(flatpak) = on_path("flatpak") {
        let installed = std::process::Command::new(&flatpak)
            .args(["info", "com.discordapp.Discord"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if installed.is_ok_and(|status| status.success()) {
            return Ok(Installation {
                program: flatpak,
                arguments: vec!["run".into(), "com.discordapp.Discord".into()],
                label: "Discord (Flatpak)".into(),
            });
        }
    }
    for location in [
        "/opt/discord/Discord",
        "/usr/share/discord/Discord",
        "/snap/bin/discord",
    ] {
        let path = PathBuf::from(location);
        if path.is_file() {
            return Ok(Installation::native(path));
        }
    }
    bail!("nenhuma instalação encontrada")
}

#[cfg(windows)]
fn discover_platform() -> Result<Installation> {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let root = PathBuf::from(local).join("Discord");
        if let Some(path) = newest_windows_install(&root) {
            return Ok(Installation::native(path));
        }
    }
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(key) {
            let root = PathBuf::from(root).join("Discord");
            if let Some(path) = newest_windows_install(&root) {
                return Ok(Installation::native(path));
            }
            let path = root.join("Discord.exe");
            if path.is_file() {
                return Ok(Installation::native(path));
            }
        }
    }
    if let Some(path) = on_path("Discord.exe") {
        return Ok(Installation::native(path));
    }
    bail!("nenhuma instalação encontrada")
}

#[cfg(any(windows, test))]
fn newest_windows_install(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let version = name
                .to_str()?
                .strip_prefix("app-")?
                .split('.')
                .map(str::parse::<u32>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            let exe = entry.path().join("Discord.exe");
            exe.is_file().then_some((version, exe))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn discover_platform() -> Result<Installation> {
    bail!("sistema não suportado")
}

pub struct Monitor(System);
impl Monitor {
    pub fn new() -> Self {
        Self(System::new())
    }

    pub fn running(&mut self) -> bool {
        self.0.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        self.0.processes().values().any(|process| {
            let name = process.name().to_string_lossy().to_ascii_lowercase();
            matches!(
                name.as_str(),
                "discord"
                    | "discord.exe"
                    | "discordcanary"
                    | "discordcanary.exe"
                    | "discordptb"
                    | "discordptb.exe"
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_versions_are_compared_numerically() -> Result<()> {
        let temp = tempfile::tempdir()?;
        for version in ["app-1.0.9", "app-1.0.10", "app-invalid"] {
            let dir = temp.path().join(version);
            std::fs::create_dir(&dir)?;
            std::fs::write(dir.join("Discord.exe"), b"")?;
        }
        assert_eq!(
            newest_windows_install(temp.path()),
            Some(temp.path().join("app-1.0.10/Discord.exe"))
        );
        Ok(())
    }
}
