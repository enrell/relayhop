use std::{
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use anyhow::Result;
use eframe::egui::{self, Color32, RichText, Vec2};

use crate::{
    media,
    protocol::VideoCodec,
    session::{self, Controls, Event},
    tray::{self, Action, State as TrayState, Tray},
};

const ACCENT: Color32 = Color32::from_rgb(143, 180, 255);
const TEAL: Color32 = Color32::from_rgb(117, 220, 202);
const DANGER: Color32 = Color32::from_rgb(255, 150, 145);
const MUTED: Color32 = Color32::from_rgb(148, 163, 196);

pub fn run() -> Result<()> {
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("relayhop")
            .with_title("RelayHop")
            .with_icon(crate::icon::window())
            .with_inner_size([580.0, 620.0])
            .with_min_inner_size([520.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RelayHop",
        native,
        Box::new(move |context| {
            context.egui_ctx.set_visuals(dark_visuals());
            Ok(Box::new(App::new()))
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
    options: session::Options,
    invite_input: String,
    current_token: Option<String>,
    current_redacted: Option<String>,
    events: Option<Receiver<Event>>,
    controls: Controls,
    status: String,
    participant: String,
    metrics_line: String,
    active: bool,
    sharing: bool,
    watching: bool,
    error: bool,
    brand: Option<egui::TextureHandle>,
    tray: Option<Tray>,
    tray_tx: Sender<Action>,
    tray_rx: Receiver<Action>,
    tray_state: Option<TrayState>,
    tray_available: bool,
    quit_requested: bool,
}

#[derive(Clone, Copy)]
enum SessionRole {
    Share,
    Watch,
    Demo,
}

impl App {
    fn new() -> Self {
        let (tray_tx, tray_rx) = mpsc::channel();
        Self {
            options: session::Options::default(),
            invite_input: String::new(),
            current_token: None,
            current_redacted: None,
            events: None,
            controls: Controls::default(),
            status: "Compartilhe sua tela por P2P. O Discord continua só para voz e convites."
                .into(),
            participant: String::new(),
            metrics_line: String::new(),
            active: false,
            sharing: false,
            watching: false,
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

    fn reset_session(&mut self) {
        self.controls = Controls::default();
        self.events = None;
        self.current_token = None;
        self.current_redacted = None;
        self.active = false;
        self.sharing = false;
        self.watching = false;
        self.error = false;
        self.participant.clear();
        self.metrics_line.clear();
    }

    fn spawn_session(&mut self, context: egui::Context, role: SessionRole, status: &str) {
        self.reset_session();
        self.controls = Controls::default();
        self.active = true;
        self.status = status.into();
        match role {
            SessionRole::Share | SessionRole::Demo => self.sharing = true,
            SessionRole::Watch => self.watching = true,
        }
        let options = self.options.clone();
        let controls = self.controls.clone();
        let invite_input = self.invite_input.clone();
        let (tx, rx) = mpsc::channel();
        self.events = Some(rx);
        std::thread::spawn(move || {
            let result = crate::runtime().and_then(|runtime| {
                runtime.block_on(async {
                    match role {
                        SessionRole::Share => {
                            session::run_host(options, controls, tx.clone()).await?;
                        }
                        SessionRole::Watch => {
                            session::run_watch(&invite_input, options, controls, tx.clone())
                                .await?;
                        }
                        SessionRole::Demo => {
                            session::run_local_demo(options, controls, tx.clone()).await?;
                        }
                    }
                    Ok::<(), anyhow::Error>(())
                })
            });
            let event = match result {
                Ok(()) => Event::Finished,
                Err(error) => Event::Error(format!("{error:#}")),
            };
            let _ = tx.send(event);
            context.request_repaint();
        });
    }

    fn start_share(&mut self, context: egui::Context) {
        if let Err(error) = self.options.validate() {
            self.reset_session();
            self.error = true;
            self.status = format!("{error:#}");
            return;
        }
        // Without the worker binary there is no transport; fail fast with the
        // actionable message instead of a bare session.
        if crate::transport::tailcat::find_worker_binary().is_err() {
            self.reset_session();
            self.error = true;
            self.status =
                "Worker Tailcat não encontrado. Compile worker/tailcat/relay (ver README).".into();
            return;
        }
        self.spawn_session(context, SessionRole::Share, "Iniciando transporte P2P…");
    }

    fn start_watch(&mut self, context: egui::Context) {
        if self.invite_input.trim().is_empty() {
            self.reset_session();
            self.error = true;
            self.status = "Cole o convite recebido do transmissor.".into();
            return;
        }
        if crate::transport::tailcat::find_worker_binary().is_err() {
            self.reset_session();
            self.error = true;
            self.status =
                "Worker Tailcat não encontrado. Compile worker/tailcat/relay (ver README).".into();
            return;
        }
        self.spawn_session(context, SessionRole::Watch, "Conectando ao transmissor…");
    }

    fn start_demo(&mut self, context: egui::Context) {
        self.spawn_session(
            context,
            SessionRole::Demo,
            "Demonstração local em andamento (sem rede pública).",
        );
    }

    fn stop(&mut self) {
        self.controls.stop_sharing.cancel();
        self.controls.stop.cancel();
        // Offline share/watch states clear immediately; a running demo thread
        // observes the same cancellation and reports Finished.
        if self.events.is_none() {
            self.reset_session();
            self.status = "Sessão encerrada; o convite expirou com a sala.".into();
        } else {
            self.status = "Encerrando demonstração…".into();
        }
    }

    fn receive(&mut self) {
        if let Some(events) = &self.events {
            for event in events.try_iter() {
                match event {
                    Event::Status(status) => self.status = status,
                    Event::InviteReady { redacted } => {
                        self.current_redacted = Some(redacted);
                    }
                    // Bearer token: shown only in the copy field, never logged.
                    Event::InviteToken(token) => {
                        self.current_token = Some(token);
                    }
                    Event::Participant(participant) => self.participant = participant,
                    Event::Metrics(line) => self.metrics_line = line,
                    Event::Finished => {
                        self.active = false;
                        self.sharing = false;
                        self.watching = false;
                        if self.status.starts_with("Demonstração local")
                            || self.status.starts_with("Encerrando")
                            || self.status.starts_with("Preparando")
                        {
                            self.status = "Demonstração concluída.".into();
                        }
                    }
                    Event::Error(error) => {
                        self.active = false;
                        self.sharing = false;
                        self.watching = false;
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
                Action::Share => {
                    if !self.active {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        self.start_share(ctx.clone());
                    }
                }
                Action::Stop => self.stop(),
                Action::Quit => {
                    self.quit_requested = true;
                    self.controls.stop_sharing.cancel();
                    self.controls.stop.cancel();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                #[cfg(target_os = "linux")]
                Action::Offline => {
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
        let state = tray::state_for(self.active, self.sharing, self.watching, self.error);
        if self.tray_state != Some(state) {
            self.tray_state = Some(state);
            if let Some(tray) = &self.tray {
                tray.update(state);
            }
        }
    }

    fn phase(&self) -> usize {
        if !self.active {
            0
        } else if self.sharing || self.watching {
            2
        } else {
            1
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
                // Keep the session alive and move the window out of sight.
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
                    ui.set_max_width(540.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        self.draw(ui, ctx);
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
            self.draw_active(ui, ctx);
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
                ui.label(RichText::new("P2P · TELA").size(12.0).color(MUTED));
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
                } else if self.sharing || self.watching {
                    TEAL
                } else {
                    ui.visuals().text_color()
                };
                ui.label(RichText::new(&self.status).color(color).size(15.0));
                if !self.participant.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&self.participant).color(MUTED).size(13.0));
                }
                if !self.metrics_line.is_empty() {
                    ui.label(RichText::new(&self.metrics_line).color(MUTED).size(13.0));
                }
                if let Some(redacted) = &self.current_redacted {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("Sala {redacted}"))
                            .color(MUTED)
                            .size(13.0),
                    );
                }
            });
    }

    fn steps(&self, ui: &mut egui::Ui) {
        let current = self.phase();
        let labels = ["Pronto", "Sala", "Ao vivo"];
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
                egui::Button::new(RichText::new("Compartilhar tela").size(17.0).strong()),
            )
            .clicked()
        {
            self.start_share(ctx.clone());
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new("Convite do transmissor:")
                .size(12.5)
                .color(MUTED),
        );
        ui.text_edit_singleline(&mut self.invite_input);
        if ui
            .add_sized(
                [ui.available_width(), 42.0],
                egui::Button::new(RichText::new("Entrar com convite").size(15.0)),
            )
            .clicked()
        {
            self.start_watch(ctx.clone());
        }
        ui.add_space(8.0);
        if ui.button("Rodar demonstração local (sem rede)").clicked() {
            self.start_demo(ctx.clone());
        }
        ui.add_space(8.0);
        ui.collapsing("Opções", |ui| self.draw_options(ui));
    }

    fn draw_active(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(token) = self.current_token.clone()
            && self.sharing
        {
            ui.label(
                RichText::new("Envie este convite ao espectador pelo Discord:")
                    .size(12.5)
                    .color(MUTED),
            );
            ui.add_space(4.0);
            let mut display = token.clone();
            ui.text_edit_multiline(&mut display);
            if ui.button("Copiar convite").clicked() {
                ctx.copy_text(token);
            }
            ui.add_space(6.0);
        }
        if self.watching && self.participant.is_empty() {
            ui.label(
                RichText::new("Conectando… o caminho (direto/relay) aparece acima quando medido.")
                    .size(12.5)
                    .color(MUTED),
            );
            ui.add_space(6.0);
        }
        ui.label(
            RichText::new(
                "Parar encerra a captura imediatamente. Fechar a janela recolhe para a bandeja.",
            )
            .size(12.5)
            .color(MUTED),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if self.tray_available && ui.button("Recolher").clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            if self.sharing && ui.button("Parar captura").clicked() {
                self.controls.stop_sharing.cancel();
            }
            if ui.button("Encerrar").clicked() {
                self.stop();
            }
        });
    }

    fn draw_options(&mut self, ui: &mut egui::Ui) {
        let sources = media::list_sources();
        egui::ComboBox::from_id_salt("source")
            .selected_text(&self.options.source_id)
            .show_ui(ui, |ui| {
                for source in &sources {
                    ui.selectable_value(
                        &mut self.options.source_id,
                        source.id.clone(),
                        &source.title,
                    );
                }
            });
        if !media::capture_available() {
            ui.label(
                RichText::new(
                    "Captura real pendente do gate de mídia; fonte sintética por enquanto.",
                )
                .size(12.0)
                .color(MUTED),
            );
        }
        let selected = match self.options.params.codec {
            VideoCodec::H264 => "H.264",
            VideoCodec::Vp8 => "VP8",
        };
        egui::ComboBox::from_id_salt("codec")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for codec in media::supported_codecs() {
                    let label = match codec {
                        VideoCodec::H264 => "H.264",
                        VideoCodec::Vp8 => "VP8",
                    };
                    ui.selectable_value(&mut self.options.params.codec, *codec, label);
                }
            });
        ui.add(egui::Slider::new(&mut self.options.params.fps, 5..=60).text("fps"));
        ui.add(
            egui::Slider::new(&mut self.options.params.bitrate_bps, 200_000..=8_000_000)
                .text("bitrate (bps)"),
        );
        ui.add(egui::Slider::new(&mut self.options.demo_secs, 1..=60).text("demonstração (s)"));
        ui.label("Mapa DERP alternativo (vazio = padrão do worker)");
        ui.text_edit_singleline(&mut self.options.derp_map);
        ui.small(format!(
            "Alvo: {}x{} · {} fps",
            self.options.params.width, self.options.params.height, self.options.params.fps
        ));
    }

    fn footer(&self, ui: &mut egui::Ui) {
        let tray_note = if self.tray_available {
            "Fechar a janela recolhe para a bandeja. Saia pelo menu da bandeja para encerrar."
        } else {
            "Bandeja indisponível neste ambiente — fechar minimiza durante a sessão."
        };
        ui.small(
            "Somente o tráfego deste app usa seu transporte. Sem VPN global, sem telemetria; logs detalhados só com RUST_LOG.",
        );
        ui.small(tray_note);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.controls.stop_sharing.cancel();
        self.controls.stop.cancel();
    }
}
