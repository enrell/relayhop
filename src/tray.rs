//! Native tray adapters. Linux uses D-Bus SNI; Windows uses its Win32 event loop.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
};

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Show,
    Start,
    Direct,
    Quit,
    /// SNI watcher went away (ksni/Linux only): keep the window visible.
    #[cfg(target_os = "linux")]
    Offline,
    /// SNI watcher is back (ksni/Linux only).
    #[cfg(target_os = "linux")]
    Online,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub label: &'static str,
    pub can_start: bool,
    pub can_direct: bool,
}

/// Short tray/menu label derived from the session phase. The full status
/// message stays in the window; the tray only mirrors the phase.
pub fn state_for(active: bool, warming: bool, direct: bool, error: bool) -> State {
    if error {
        State {
            label: "Erro — abra o RelayHop",
            can_start: true,
            can_direct: false,
        }
    } else if direct {
        State {
            label: "Conexão direta",
            can_start: false,
            can_direct: false,
        }
    } else if warming {
        State {
            label: "Pelo Tor — aguarde carregar",
            can_start: false,
            can_direct: true,
        }
    } else if active {
        State {
            label: "Conectando…",
            can_start: false,
            can_direct: false,
        }
    } else {
        State {
            label: "Pronto",
            can_start: true,
            can_direct: false,
        }
    }
}

pub type Dispatch = Arc<dyn Fn(Action) + Send + Sync>;

pub fn dispatcher(
    tx: Sender<Action>,
    ctx: egui::Context,
    quit_requested: Arc<AtomicBool>,
    stop: tokio_util::sync::CancellationToken,
) -> Dispatch {
    Arc::new(move |action| {
        if matches!(action, Action::Quit) {
            // The tray callback runs outside egui's update loop. Publish the
            // intent before queueing the close command so a pending native
            // close event cannot be mistaken for the normal "hide to tray"
            // window close path. Cancel the session here as well: when the
            // window is hidden in the tray, egui may never run another frame,
            // so `drain_tray` would never observe the Quit action.
            quit_requested.store(true, Ordering::Release);
            stop.cancel();
        }
        let _ = tx.send(action);
        // Queue commands before waking the event loop. On Windows an invisible
        // viewport may not run another frame for a plain repaint request, so
        // waiting for `drain_tray` made Quit appear to do nothing.
        match action {
            Action::Show => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Action::Quit => {
                #[cfg(windows)]
                platform::post_close_to_window();
                // A hidden Windows viewport may not repaint until it becomes
                // visible again. Wake it and close it in the same batch.
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                // Final guarantee: the hidden viewport may never produce the
                // frame needed to consume the commands above. If the graceful
                // close works, the process exits first and this watchdog dies
                // with it; otherwise force termination so "Sair" always ends
                // the process. The Tor worker watches the parent stdin pipe
                // and exits on EOF, so no orphan is left behind.
                if !cfg!(test) {
                    std::thread::spawn(|| {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        std::process::exit(0);
                    });
                }
            }
            _ => {}
        }
        ctx.request_repaint();
    })
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use ksni::{blocking::TrayMethods, menu::StandardItem};

    struct Item {
        state: State,
        dispatch: Dispatch,
    }

    impl ksni::Tray for Item {
        fn id(&self) -> String {
            "relayhop".into()
        }
        fn title(&self) -> String {
            format!("RelayHop — {}", self.state.label)
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            vec![ksni::Icon {
                width: crate::icon::TRAY_SIZE as i32,
                height: crate::icon::TRAY_SIZE as i32,
                data: crate::icon::argb(),
            }]
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "RelayHop".into(),
                description: self.state.label.into(),
                icon_pixmap: self.icon_pixmap(),
                ..Default::default()
            }
        }
        fn activate(&mut self, _: i32, _: i32) {
            (self.dispatch)(Action::Show);
        }
        fn watcher_offline(&self, _: ksni::OfflineReason) -> bool {
            (self.dispatch)(Action::Offline);
            true
        }
        fn watcher_online(&self) {
            (self.dispatch)(Action::Online);
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            vec![
                StandardItem {
                    label: self.state.label.into(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: "Mostrar RelayHop".into(),
                    activate: Box::new(|this: &mut Self| (this.dispatch)(Action::Show)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Abrir Discord".into(),
                    enabled: self.state.can_start,
                    activate: Box::new(|this: &mut Self| (this.dispatch)(Action::Start)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Usar conexão direta".into(),
                    enabled: self.state.can_direct,
                    activate: Box::new(|this: &mut Self| (this.dispatch)(Action::Direct)),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: "Sair do RelayHop".into(),
                    activate: Box::new(|this: &mut Self| (this.dispatch)(Action::Quit)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    pub struct Tray(ksni::blocking::Handle<Item>);
    impl Tray {
        pub fn new(state: State, dispatch: Dispatch) -> anyhow::Result<Self> {
            Ok(Self(Item { state, dispatch }.spawn()?))
        }
        pub fn update(&self, state: State) {
            self.0.update(|item| item.state = state);
        }
        pub fn is_closed(&self) -> bool {
            self.0.is_closed()
        }
    }
    impl Drop for Tray {
        fn drop(&mut self) {
            self.0.shutdown();
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ptr;

    use tray_icon::{
        MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, PostMessageW, SW_RESTORE, SW_SHOW,
        SetForegroundWindow, ShowWindow, WM_CLOSE,
    };
    use windows_sys::core::BOOL;

    struct CloseSearch {
        process_id: u32,
        relayhop_window: HWND,
        candidates: Vec<HWND>,
    }

    unsafe extern "system" fn find_relayhop_window(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` points to the stack-owned `CloseSearch` passed to
        // EnumWindows and remains valid until that call returns.
        let search = unsafe { &mut *(data as *mut CloseSearch) };
        let mut process_id = 0;
        if unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) } == 0
            || process_id != search.process_id
        {
            return 1;
        }
        if search.candidates.len() < 32 {
            search.candidates.push(hwnd);
        }

        let mut title = [0u16; 128];
        let length = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
        if length > 0 && String::from_utf16_lossy(&title[..length as usize]) == "RelayHop" {
            search.relayhop_window = hwnd;
        }
        1
    }

    pub(super) fn post_close_to_window() {
        let mut search = CloseSearch {
            process_id: std::process::id(),
            relayhop_window: ptr::null_mut(),
            candidates: Vec::new(),
        };
        // SAFETY: the callback only reads the current process id and writes to
        // the stack-owned search value; EnumWindows invokes it synchronously.
        unsafe {
            EnumWindows(
                Some(find_relayhop_window),
                &mut search as *mut CloseSearch as LPARAM,
            );
            // Prefer the exact title match, but fall back to every top-level
            // window of this process: the tray helper window may be enumerated
            // first, and posting only to it would lose the close request.
            let mut targets = Vec::new();
            if !search.relayhop_window.is_null() {
                targets.push(search.relayhop_window);
            }
            for hwnd in search.candidates {
                if hwnd != search.relayhop_window {
                    targets.push(hwnd);
                }
            }
            for hwnd in targets {
                // Unhide natively so winit observes the close on a visible
                // window even when egui cannot run a frame to apply
                // `ViewportCommand::Visible(true)`.
                ShowWindow(hwnd, SW_SHOW);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                // WM_CLOSE is delivered through winit's native event path,
                // including when the window is hidden and egui cannot repaint.
                let _ = PostMessageW(hwnd, WM_CLOSE, 0, 0);
                if hwnd == search.relayhop_window {
                    break;
                }
            }
        }
    }

    pub struct Tray {
        icon: TrayIcon,
        status: MenuItem,
        start: MenuItem,
        direct: MenuItem,
    }
    impl Tray {
        pub fn new(state: State, dispatch: Dispatch) -> anyhow::Result<Self> {
            let menu = Menu::new();
            let status = MenuItem::new(state.label, false, None);
            let show = MenuItem::with_id("show", "Mostrar RelayHop", true, None);
            let start = MenuItem::with_id("start", "Abrir Discord", state.can_start, None);
            let direct = MenuItem::with_id("direct", "Usar conexão direta", state.can_direct, None);
            let quit = MenuItem::with_id("quit", "Sair do RelayHop", true, None);
            menu.append_items(&[
                &status,
                &PredefinedMenuItem::separator(),
                &show,
                &start,
                &direct,
                &PredefinedMenuItem::separator(),
                &quit,
            ])?;
            let click_dispatch = dispatch.clone();
            TrayIconEvent::set_event_handler(Some(move |event| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    click_dispatch(Action::Show);
                }
            }));
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let action = match event.id.as_ref() {
                    "show" => Action::Show,
                    "start" => Action::Start,
                    "direct" => Action::Direct,
                    "quit" => Action::Quit,
                    _ => return,
                };
                dispatch(action);
            }));
            let icon = TrayIconBuilder::new()
                .with_id("relayhop")
                .with_icon(tray_icon::Icon::from_rgba(
                    crate::icon::TRAY_RGBA.to_vec(),
                    crate::icon::TRAY_SIZE,
                    crate::icon::TRAY_SIZE,
                )?)
                .with_tooltip(format!("RelayHop — {}", state.label))
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()?;
            Ok(Self {
                icon,
                status,
                start,
                direct,
            })
        }
        pub fn update(&self, state: State) {
            self.status.set_text(state.label);
            self.start.set_enabled(state.can_start);
            self.direct.set_enabled(state.can_direct);
            let _ = self
                .icon
                .set_tooltip(Some(format!("RelayHop — {}", state.label)));
        }
        pub fn is_closed(&self) -> bool {
            false
        }
    }
    impl Drop for Tray {
        fn drop(&mut self) {
            TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
            MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use super::*;
    pub struct Tray;
    impl Tray {
        pub fn new(_: State, _: Dispatch) -> anyhow::Result<Self> {
            anyhow::bail!("bandeja não suportada")
        }
        pub fn update(&self, _: State) {}
        pub fn is_closed(&self) -> bool {
            true
        }
    }
}

pub use platform::Tray;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_dispatch_queues_close_before_the_next_frame() {
        let context = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let quit_requested = Arc::new(AtomicBool::new(false));
        let stop = tokio_util::sync::CancellationToken::new();
        dispatcher(tx, context.clone(), quit_requested.clone(), stop.clone())(Action::Quit);

        assert_eq!(rx.recv().unwrap(), Action::Quit);
        assert!(quit_requested.load(Ordering::Acquire));
        assert!(stop.is_cancelled());
        let output = context.run(Default::default(), |_| {});
        assert!(output.viewport_output.values().any(|viewport| {
            viewport
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Visible(true)))
                && viewport
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::Close))
        }));
    }

    #[test]
    fn tray_state_mirrors_session_phase() {
        assert_eq!(
            state_for(false, false, false, false),
            State {
                label: "Pronto",
                can_start: true,
                can_direct: false
            }
        );
        assert_eq!(
            state_for(true, false, false, false),
            State {
                label: "Conectando…",
                can_start: false,
                can_direct: false
            }
        );
        assert_eq!(
            state_for(true, true, false, false),
            State {
                label: "Pelo Tor — aguarde carregar",
                can_start: false,
                can_direct: true
            }
        );
        assert_eq!(
            state_for(true, false, true, false),
            State {
                label: "Conexão direta",
                can_start: false,
                can_direct: false
            }
        );
        assert_eq!(
            state_for(false, false, false, true),
            State {
                label: "Erro — abra o RelayHop",
                can_start: true,
                can_direct: false
            }
        );
    }
}
