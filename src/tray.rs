//! Native tray adapters. Linux uses D-Bus SNI; Windows uses its Win32 event loop.
use std::sync::{Arc, mpsc::Sender};

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Show,
    Share,
    Stop,
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
    pub can_share: bool,
    pub can_stop: bool,
}

/// Short tray/menu label derived from the session phase. The full status
/// message stays in the window; the tray only mirrors the phase.
pub fn state_for(active: bool, sharing: bool, watching: bool, error: bool) -> State {
    if error {
        State {
            label: "Erro — abra o RelayHop",
            can_share: true,
            can_stop: false,
        }
    } else if sharing {
        State {
            label: "Compartilhando tela",
            can_share: false,
            can_stop: true,
        }
    } else if watching {
        State {
            label: "Assistindo",
            can_share: false,
            can_stop: true,
        }
    } else if active {
        State {
            label: "Preparando…",
            can_share: false,
            can_stop: true,
        }
    } else {
        State {
            label: "Pronto",
            can_share: true,
            can_stop: false,
        }
    }
}

pub type Dispatch = Arc<dyn Fn(Action) + Send + Sync>;

pub fn dispatcher(tx: Sender<Action>, ctx: egui::Context) -> Dispatch {
    Arc::new(move |action| {
        let _ = tx.send(action);
        // Enqueue the restore here too so a hidden window comes back even
        // if the event loop is busy; Offline is restored by drain_tray.
        if matches!(action, Action::Show) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
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
                    label: "Compartilhar tela".into(),
                    enabled: self.state.can_share,
                    activate: Box::new(|this: &mut Self| (this.dispatch)(Action::Share)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Parar compartilhamento".into(),
                    enabled: self.state.can_stop,
                    activate: Box::new(|this: &mut Self| (this.dispatch)(Action::Stop)),
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
    use tray_icon::{
        MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };

    pub struct Tray {
        icon: TrayIcon,
        status: MenuItem,
        share: MenuItem,
        stop: MenuItem,
    }
    impl Tray {
        pub fn new(state: State, dispatch: Dispatch) -> anyhow::Result<Self> {
            let menu = Menu::new();
            let status = MenuItem::new(state.label, false, None);
            let show = MenuItem::with_id("show", "Mostrar RelayHop", true, None);
            let share = MenuItem::with_id("share", "Compartilhar tela", state.can_share, None);
            let stop = MenuItem::with_id("stop", "Parar compartilhamento", state.can_stop, None);
            let quit = MenuItem::with_id("quit", "Sair do RelayHop", true, None);
            menu.append_items(&[
                &status,
                &PredefinedMenuItem::separator(),
                &show,
                &share,
                &stop,
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
                    "share" => Action::Share,
                    "stop" => Action::Stop,
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
                share,
                stop,
            })
        }
        pub fn update(&self, state: State) {
            self.status.set_text(state.label);
            self.share.set_enabled(state.can_share);
            self.stop.set_enabled(state.can_stop);
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
    fn tray_state_mirrors_session_phase() {
        assert_eq!(
            state_for(false, false, false, false),
            State {
                label: "Pronto",
                can_share: true,
                can_stop: false
            }
        );
        assert_eq!(
            state_for(true, false, false, false),
            State {
                label: "Preparando…",
                can_share: false,
                can_stop: true
            }
        );
        assert_eq!(
            state_for(true, true, false, false),
            State {
                label: "Compartilhando tela",
                can_share: false,
                can_stop: true
            }
        );
        assert_eq!(
            state_for(true, false, true, false),
            State {
                label: "Assistindo",
                can_share: false,
                can_stop: true
            }
        );
        assert_eq!(
            state_for(false, false, false, true),
            State {
                label: "Erro — abra o RelayHop",
                can_share: true,
                can_stop: false
            }
        );
    }
}
