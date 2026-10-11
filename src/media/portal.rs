//! XDG ScreenCast portal: user consent + the PipeWire node to capture.
//!
//! The portal is the only way to capture the desktop on Wayland (and the
//! supported way under X11 too). The flow is:
//!
//! 1. `CreateSession`
//! 2. `SelectSources` (monitors and/or windows, cursor mode, restore token)
//! 3. `Start` — the compositor shows its own picker; the user chooses there
//! 4. the returned stream carries the PipeWire node id that `pipewiresrc` needs
//!
//! The portal session stays open for as long as the returned guard is alive:
//! dropping it ends the screen cast (and revoking it in the compositor stops
//! the frames, which the pipeline reports as an error/EOS).
//!
//! Nothing here reads the screen by itself and nothing is captured without the
//! explicit selection in step 3.

use anyhow::{Context, Result, anyhow, bail};
use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, StartCastOptions,
};
use ashpd::desktop::{PersistMode, Session};
use ashpd::enumflags2::BitFlags;

use super::SourceKind;

/// A live screen-cast session plus the node the encoder must read.
pub struct CaptureSelection {
    pub node_id: u32,
    pub size: Option<(i32, i32)>,
    pub restore_token: Option<String>,
    pub label: String,
    /// Kept alive on purpose: dropping it closes the portal session and the
    /// compositor stops sending frames.
    _session: Session<Screencast>,
    _proxy: Screencast,
}

impl std::fmt::Debug for CaptureSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureSelection")
            .field("node_id", &self.node_id)
            .field("size", &self.size)
            .field("label", &self.label)
            .finish()
    }
}

fn source_types(kind: SourceKind) -> BitFlags<SourceType> {
    match kind {
        SourceKind::Window => SourceType::Window.into(),
        // Screen (and anything else) offers monitors plus windows so the user
        // can pick either in the compositor's own picker.
        _ => SourceType::Monitor | SourceType::Window,
    }
}

/// Ask the portal for one screen/window stream. `restore_token` comes from a
/// previous selection (persisted by the caller) and skips the dialog when the
/// compositor still honors it.
pub async fn select_source(
    kind: SourceKind,
    restore_token: Option<&str>,
) -> Result<CaptureSelection> {
    let proxy = Screencast::new()
        .await
        .context("portal ScreenCast indisponível (xdg-desktop-portal instalado e rodando?)")?;
    let session = proxy
        .create_session(Default::default())
        .await
        .context("o portal recusou criar a sessão de captura")?;
    let options = SelectSourcesOptions::default()
        .set_multiple(false)
        .set_cursor_mode(CursorMode::Embedded)
        .set_sources(source_types(kind))
        .set_persist_mode(PersistMode::Application)
        .set_restore_token(restore_token);
    proxy
        .select_sources(&session, options)
        .await
        .context("o portal recusou a seleção de fontes")?;
    // The compositor shows its picker here; this is the user's consent.
    // (`restore_token` is honored through SelectSources; Start only needs the
    // session handle.)
    let request = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .context("o portal recusou iniciar a captura")?;
    let streams = request
        .response()
        .context("seleção cancelada ou recusada no portal")?;
    let Some(stream) = streams.streams().first() else {
        bail!("o portal não devolveu nenhuma fonte (seleção vazia)");
    };
    let node_id = stream.pipe_wire_node_id();
    if node_id == 0 {
        bail!("o portal devolveu um nó PipeWire inválido");
    }
    let label = match stream.source_type() {
        Some(SourceType::Window) => "janela".to_string(),
        Some(SourceType::Monitor) => "monitor".to_string(),
        _ => "fonte".to_string(),
    };
    Ok(CaptureSelection {
        node_id,
        size: stream.size(),
        restore_token: streams.restore_token().map(str::to_string),
        label,
        _session: session,
        _proxy: proxy,
    })
}

/// Run the portal handshake on a dedicated thread (the UI is synchronous) and
/// hand back the selection. The portal dialog blocks until the user answers.
pub fn spawn_selection(
    kind: SourceKind,
    restore_token: Option<String>,
) -> std::sync::mpsc::Receiver<Result<CaptureSelection>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("relayhop-portal".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let result = match runtime {
                Ok(runtime) => runtime.block_on(select_source(kind, restore_token.as_deref())),
                Err(error) => Err(anyhow!(
                    "não foi possível iniciar o runtime do portal: {error}"
                )),
            };
            let _ = tx.send(result);
        })
        .ok();
    rx
}

/// True when a ScreenCast portal is reachable (does not open any session).
pub async fn available() -> bool {
    Screencast::new().await.is_ok()
}

/// Path of the persisted restore token (per user, per app data dir).
fn token_path() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("RELAYHOP_DATA_DIR") {
        return Some(std::path::PathBuf::from(dir).join("portal.token"));
    }
    let dirs = directories::ProjectDirs::from("", "", "relayhop")?;
    Some(dirs.data_local_dir().join("portal.token"))
}

/// Restore token from a previous selection: the compositor then skips the
/// picker when it still honors it. Never contains a room secret.
pub fn load_restore_token() -> Option<String> {
    let path = token_path()?;
    let token = std::fs::read_to_string(path).ok()?;
    let token = token.trim();
    if token.is_empty() || token.len() > 4096 {
        return None;
    }
    Some(token.to_string())
}

/// Persist the restore token so the next run can skip the picker.
pub fn store_restore_token(token: &str) {
    let Some(path) = token_path() else {
        return;
    };
    if token.is_empty() || token.len() > 4096 {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, token);
}
