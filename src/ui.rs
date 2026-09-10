use std::{
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

use anyhow::Result;
use eframe::egui::{self, Color32, RichText, Vec2};

use crate::{
    session::{self, Controls, Event, Options},
    tray::{self, Action, State as TrayState, Tray},
};

const ACCENT: Color32 = Color32::from_rgb(143, 180, 255);
const TEAL: Color32 = Color32::from_rgb(117, 220, 202);
const DANGER: Color32 = Color32::from_rgb(255, 150, 145);
const MUTED: Color32 = Color32::from_rgb(148, 163, 196);

pub fn run(options: Options) -> Result<()> {
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("relayhop")
            .with_title("RelayHop")
            .with_icon(crate::icon::window())
            .with_inner_size([560.0, 560.0])
            .with_min_inner_size([500.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RelayHop",
        native,
        Box::new(move |context| {
            context.egui_ctx.set_visuals(dark_visuals());
            Ok(Box::new(App::new(options)))
        }),
    )
    .map_err(|error| anyhow::anyhow!("interface gráfica: {error}"))
}

fn dark_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.window_corner_radius = egui::CornerRadius::same(14);
    visuals.menu_corner_radius = egui::CornerRadius::same(10);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(46, 64, 102);
    visuals.selection.bg_fill = Color32::from_rgb(58, 84, 134);
    visuals
}

struct App {
    options: Options,
    path: String,
    events: Option<Receiver<Event>>,
    controls: Controls,
    status: String,
    active: bool,
    warming: bool,
    direct: bool,
    deadline: Option<Instant>,
    error: bool,
    brand: Option<egui::TextureHandle>,
    tray: Option<Tray>,
    tray_tx: Sender<Action>,
    tray_rx: Receiver<Action>,
    tray_state: Option<TrayState>,
    tray_available: bool,
    quit_requested: bool,
}

impl App {
    fn new(options: Options) -> Self {
        let path = options
            .discord
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let (tray_tx, tray_rx) = mpsc::channel();
        Self {
            options,
            path,
            events: None,
            controls: Controls::default(),
            status: "Um salto pelo Tor na abertura. Sua conexão normal depois.".into(),
            active: false,
            warming: false,
            direct: false,
            deadline: None,
            error: false,
            brand: None,
            tray: None,
            tray_tx,
            tray_rx,
            tray_state: None,
            tray_available: true,
            quit_requested: false,
        }
    }

    fn start(&mut self, context: egui::Context) {
        self.controls = Controls::default();
        self.active = true;
        self.warming = false;
        self.direct = false;
        self.error = false;
        self.deadline = None;
        self.options.discord = (!self.path.trim().is_empty()).then(|| self.path.trim().into());
        let options = self.options.clone();
        let controls = self.controls.clone();
        let (tx, rx) = mpsc::channel();
        self.events = Some(rx);
        std::thread::spawn(move || {
            let result = crate::runtime()
                .and_then(|runtime| runtime.block_on(session::run(options, controls, tx.clone())));
            let event = match result {
                Ok(()) => Event::Finished,
                Err(error) => Event::Error(format!("{error:#}")),
            };
            let _ = tx.send(event);
            context.request_repaint();
        });
    }

    fn receive(&mut self) {
        if let Some(events) = &self.events {
            for event in events.try_iter() {
                match event {
                    Event::Status(status) => self.status = status,
                    Event::Warming(deadline) => {
                        self.warming = true;
                        self.deadline = deadline;
                        self.status =
                            "Discord conectado pelo Tor. Aguarde a tela inicial carregar.".into();
                    }
                    Event::Direct => {
                        self.direct = true;
                        self.warming = false;
                        self.status = "Conexão direta ativa. O cliente Tor foi encerrado.".into();
                    }
                    Event::Finished => {
                        self.active = false;
                        self.warming = false;
                        self.status = "Sessão encerrada.".into();
                    }
                    Event::Error(error) => {
                        self.active = false;
                        self.warming = false;
                        self.error = true;
                        self.status = error;
                    }
                }
            }
        }
    }

    fn drain_tray(&mut self, ctx: &egui::Context) {
        for action in self.tray_rx.try_iter().collect::<Vec<_>>() {
            match action {
                Action::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Action::Start => {
                    if !self.active {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        self.start(ctx.clone());
                    }
                }
                Action::Direct => self.controls.direct.cancel(),
                Action::Quit => {
                    self.quit_requested = true;
                    self.controls.stop.cancel();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                #[cfg(target_os = "linux")]
                Action::Offline => {
                    // No host claims the tray icon: keep the window around.
                    self.tray_available = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                }
                #[cfg(target_os = "linux")]
                Action::Online => self.tray_available = true,
            }
        }
    }

    fn ensure_tray(&mut self, ctx: &egui::Context) {
        if self.tray.is_some() || !matches!(std::env::consts::OS, "linux" | "windows") {
            return;
        }
        let dispatch = tray::dispatcher(self.tray_tx.clone(), ctx.clone());
        match Tray::new(tray::state_for(false, false, false, false), dispatch) {
            Ok(created) => {
                // A host may still report the icon as closed (no watcher/host).
                self.tray_available = !created.is_closed();
                self.tray = Some(created);
            }
            Err(error) => {
                tracing::debug!(%error, "tray indisponível; seguindo sem bandeja");
                self.tray_available = false;
            }
        }
    }

    fn sync_tray(&mut self) {
        let state = tray::state_for(self.active, self.warming, self.direct, self.error);
        if self.tray_state != Some(state) {
            self.tray_state = Some(state);
            if let Some(tray) = &self.tray {
                tray.update(state);
            }
        }
    }

    fn phase(&self) -> usize {
        if self.direct {
            3
        } else if self.warming {
            2
        } else if self.active {
            1
        } else {
            0
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.receive();
        self.ensure_tray(ctx);
        self.drain_tray(ctx);
        self.sync_tray();
        if self.brand.is_none() {
            self.brand = Some(crate::icon::texture(ctx));
        }
        if self.active {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if ctx.input(|input| input.viewport().close_requested()) {
            if self.quit_requested {
                // Allow the window (and event loop) to close.
            } else if self.tray_available {
                // The forwarder is still needed: keep the process alive and
                // move the window out of sight instead of breaking every
                // proxied Discord connection. Both commands are sent because
                // each backend honors a different one (Wayland ignores hide,
                // some compositors ignore minimize); on XWayland both work.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            } else if self.active {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(520.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        self.draw(ui, ctx)
                    });
                });
            });
        });
    }
}

impl App {
    fn draw(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(14.0);
        self.hero(ui);
        ui.add_space(12.0);
        self.status_card(ui);
        ui.add_space(12.0);
        if self.active {
            self.draw_active(ui);
        } else {
            self.draw_idle(ui, ctx);
        }
        ui.add_space(10.0);
        ui.separator();
        self.footer(ui);
    }

    fn hero(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(brand) = &self.brand {
                ui.add(egui::Image::new((brand.id(), Vec2::new(52.0, 52.0))).corner_radius(12.0));
            }
            ui.vertical(|ui| {
                ui.label(RichText::new("RelayHop").size(32.0).color(ACCENT).strong());
                ui.label(
                    RichText::new("TOR TEMPORÁRIO · DISCORD")
                        .size(12.0)
                        .color(MUTED),
                );
            });
        });
    }

    fn status_card(&self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style())
            .corner_radius(12)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.steps(ui);
                ui.add_space(10.0);
                let color = if self.error {
                    DANGER
                } else if self.direct {
                    TEAL
                } else {
                    ui.visuals().text_color()
                };
                ui.label(RichText::new(&self.status).color(color).size(15.0));
                if self.warming && !self.direct {
                    ui.add_space(6.0);
                    if let Some(deadline) = self.deadline {
                        let remaining =
                            deadline.saturating_duration_since(Instant::now()).as_secs();
                        ui.label(
                            RichText::new(format!("Conexão direta em {remaining} s")).color(MUTED),
                        );
                        ui.add(egui::ProgressBar::new(progress(self.deadline)).show_percentage());
                    } else {
                        ui.label(
                            RichText::new("Modo manual: você decide a hora da troca.").color(MUTED),
                        );
                    }
                }
            });
    }

    fn steps(&self, ui: &mut egui::Ui) {
        let current = self.phase();
        let labels = ["Pronto", "Tor", "Discord", "Direta"];
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            for (index, label) in labels.iter().enumerate() {
                let reached = index <= current;
                let (fill, text) = if index == current && current > 0 {
                    (ACCENT, Color32::from_rgb(10, 16, 32))
                } else if reached {
                    (Color32::from_rgb(46, 64, 102), Color32::WHITE)
                } else {
                    (Color32::from_rgb(30, 38, 58), MUTED)
                };
                egui::Frame::new()
                    .fill(fill)
                    .corner_radius(16)
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 10,
                        top: 4,
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("{index} · {label}"))
                                .size(12.0)
                                .color(text),
                        );
                    });
            }
        });
    }

    fn draw_idle(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if ui
            .add_sized(
                [ui.available_width(), 48.0],
                egui::Button::new(RichText::new("Abrir Discord").size(17.0).strong()),
            )
            .clicked()
        {
            self.start(ctx.clone());
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new("Saia do Discord pela bandeja antes de começar. Uma instância aberta ignora a nova conexão.")
                .size(12.5)
                .color(MUTED),
        );
        ui.add_space(8.0);
        ui.collapsing("Opções", |ui| self.draw_options(ui));
    }

    fn draw_active(&self, ui: &mut egui::Ui) {
        if self.warming {
            if ui
                .add_sized(
                    [ui.available_width(), 46.0],
                    egui::Button::new(
                        RichText::new("Discord carregou — usar conexão direta")
                            .size(15.0)
                            .strong(),
                    ),
                )
                .clicked()
            {
                self.controls.direct.cancel();
            }
            ui.add_space(8.0);
        } else if !self.direct {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Preparando o salto…");
            });
            ui.add_space(8.0);
        }
        if self.direct || self.warming {
            ui.label(
                RichText::new("Deixe o RelayHop na bandeja enquanto usa o Discord. Fechar a janela não encerra a sessão.")
                    .size(12.5)
                    .color(MUTED),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if self.tray_available && ui.button("Recolher").clicked() {
                    // Same pair as the close handler: each backend honors one.
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                if ui.button("Cancelar").clicked() {
                    self.controls.stop.cancel();
                }
            });
            ui.collapsing("Encerrar manualmente", |ui| {
                ui.label(
                    "Isso interrompe as conexões encaminhadas do Discord. Para continuar sem RelayHop, reabra o Discord normalmente.",
                );
                if ui.button("Encerrar RelayHop").clicked() {
                    self.controls.stop.cancel();
                }
            });
        } else if ui.button("Cancelar").clicked() {
            self.controls.stop.cancel();
        }
    }

    fn draw_options(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Saída Tor");
            egui::ComboBox::from_id_salt("country")
                .selected_text(&self.options.country)
                .show_ui(ui, |ui| {
                    for country in ["US", "DE", "NL", "CA", "FR", "GB"] {
                        ui.selectable_value(&mut self.options.country, country.into(), country);
                    }
                });
        });
        ui.checkbox(&mut self.options.manual, "Trocar apenas quando eu clicar");
        ui.add_enabled(
            !self.options.manual,
            egui::Slider::new(&mut self.options.warmup_secs, 10..=600).text("segundos pelo Tor"),
        );
        ui.label("Executável do Discord (vazio = detectar)");
        ui.text_edit_singleline(&mut self.path);
    }

    fn footer(&self, ui: &mut egui::Ui) {
        let tray_note = if self.tray_available {
            "Fechar a janela recolhe para a bandeja. Saia pelo menu da bandeja para encerrar."
        } else {
            "Bandeja indisponível neste ambiente — fechar minimiza durante a sessão."
        };
        ui.small(
            "Sem conta, VPN global ou certificados adicionais. A disponibilidade depende das saídas Tor e do Discord.",
        );
        ui.small(tray_note);
    }
}

fn progress(deadline: Option<Instant>) -> f32 {
    // The warmup length isn't threaded through events; approximate the bar
    // from a stable 60 s window so the UI never divides by zero.
    deadline
        .map(|end| {
            let remaining = end.saturating_duration_since(Instant::now()).as_secs_f32();
            (1.0 - remaining / 60.0).clamp(0.0, 1.0)
        })
        .unwrap_or(0.0)
}

impl Drop for App {
    fn drop(&mut self) {
        self.controls.stop.cancel();
    }
}
