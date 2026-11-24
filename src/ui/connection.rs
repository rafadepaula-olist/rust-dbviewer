use crate::models::ConnectionConfig;
use crate::ui::theme;
use egui::{Color32, RichText, Rounding, Stroke, Ui};
use uuid::Uuid;

/// Actions emitted by the connection screen.
#[derive(Debug, Clone)]
pub enum ConnectionAction {
    Test(ConnectionConfig),
    Connect(ConnectionConfig),
    Save(ConnectionConfig),
    Delete(String),
}

/// State for the connection management screen.
pub struct ConnectionScreenState {
    pub saved_connections: Vec<ConnectionConfig>,
    pub selected_id: Option<String>,
    pub form: ConnectionConfig,
    pub port_text: String,
    pub database_text: String,
    pub timeout_text: String,
    pub show_password: bool,
    pub status_banner: Option<(bool, String)>, // (is_error, message)
    pub testing: bool,
    pub connecting: bool,
}

impl ConnectionScreenState {
    pub fn new(saved_connections: Vec<ConnectionConfig>) -> Self {
        let initial = saved_connections
            .first()
            .cloned()
            .unwrap_or_default();
        let selected_id = if saved_connections.is_empty() {
            None
        } else {
            Some(initial.id.clone())
        };
        let port_text = initial.port.to_string();
        let database_text = initial.database.clone().unwrap_or_default();
        let timeout_text = initial.connect_timeout_secs.to_string();

        Self {
            saved_connections,
            selected_id,
            form: initial,
            port_text,
            database_text,
            timeout_text,
            show_password: false,
            status_banner: None,
            testing: false,
            connecting: false,
        }
    }

    /// Load a saved connection into the form fields.
    pub fn load_into_form(&mut self, config: &ConnectionConfig) {
        self.selected_id = Some(config.id.clone());
        self.form = config.clone();
        self.port_text = config.port.to_string();
        self.database_text = config.database.clone().unwrap_or_default();
        self.timeout_text = config.connect_timeout_secs.to_string();
        self.status_banner = None;
    }

    /// Reset form to a blank new MySQL/MariaDB connection.
    pub fn reset_new_form(&mut self) {
        let fresh = ConnectionConfig {
            id: Uuid::new_v4().to_string(),
            name: String::new(),
            ..Default::default()
        };
        self.selected_id = None;
        self.port_text = fresh.port.to_string();
        self.database_text.clear();
        self.timeout_text = fresh.connect_timeout_secs.to_string();
        self.form = fresh;
        self.status_banner = None;
    }

    /// Build a validated `ConnectionConfig` from current form inputs.
    pub fn build_config(&self) -> Result<ConnectionConfig, String> {
        let host = self.form.host.trim();
        if host.is_empty() {
            return Err("Host is required.".to_string());
        }
        let port: u16 = self
            .port_text
            .trim()
            .parse()
            .map_err(|_| "Port must be a valid number between 1 and 65535.".to_string())?;
        if port == 0 {
            return Err("Port must be greater than 0.".to_string());
        }
        let timeout: u64 = self
            .timeout_text
            .trim()
            .parse()
            .unwrap_or(10)
            .max(1);

        let mut cfg = self.form.clone();
        cfg.host = host.to_string();
        cfg.port = port;
        cfg.username = self.form.username.trim().to_string();
        cfg.connect_timeout_secs = timeout;
        let db = self.database_text.trim();
        cfg.database = if db.is_empty() {
            None
        } else {
            Some(db.to_string())
        };
        if cfg.name.trim().is_empty() {
            cfg.name = format!("{}@{}:{}", cfg.username, cfg.host, cfg.port);
        } else {
            cfg.name = cfg.name.trim().to_string();
        }
        Ok(cfg)
    }

    pub fn is_busy(&self) -> bool {
        self.testing || self.connecting
    }

    /// Render the connection screen (saved connections sidebar + connection form).
    pub fn show(&mut self, ctx: &egui::Context) -> Option<ConnectionAction> {
        let mut action: Option<ConnectionAction> = None;

        // Left sidebar: saved connections list
        egui::SidePanel::left("connection_sidebar")
            .resizable(true)
            .default_width(270.0)
            .min_width(210.0)
            .max_width(420.0)
            .frame(
                egui::Frame::none()
                    .fill(theme::BG_SIDEBAR)
                    .inner_margin(egui::Margin::same(12.0))
                    .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Saved Connections")
                            .strong()
                            .size(14.5)
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("+ New").clicked() {
                            self.reset_new_form();
                        }
                    });
                });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                if self.saved_connections.is_empty() {
                    ui.label(
                        RichText::new("No saved connections yet.\nFill in the form and click Save.")
                            .color(theme::TEXT_MUTED)
                            .size(12.5),
                    );
                } else {
                    let mut load_idx: Option<usize> = None;
                    let mut connect_idx: Option<usize> = None;
                    let mut delete_id: Option<String> = None;

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (idx, conn) in self.saved_connections.iter().enumerate() {
                            let is_selected = self.selected_id.as_deref() == Some(conn.id.as_str());
                            let fill = if is_selected {
                                theme::BG_SELECTED
                            } else {
                                Color32::TRANSPARENT
                            };
                            let stroke = if is_selected {
                                Stroke::new(1.0_f32, theme::ACCENT)
                            } else {
                                Stroke::new(1.0_f32, Color32::TRANSPARENT)
                            };

                            let frame_resp = egui::Frame::none()
                                .fill(fill)
                                .stroke(stroke)
                                .rounding(Rounding::same(4.0))
                                .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.label(
                                                RichText::new(&conn.name)
                                                    .strong()
                                                    .color(if is_selected {
                                                        theme::ACCENT
                                                    } else {
                                                        theme::TEXT_PRIMARY
                                                    }),
                                            );
                                            let subtitle = match &conn.database {
                                                Some(db) if !db.is_empty() => format!(
                                                    "{}@{}:{}/{}",
                                                    conn.username, conn.host, conn.port, db
                                                ),
                                                _ => format!(
                                                    "{}@{}:{}",
                                                    conn.username, conn.host, conn.port
                                                ),
                                            };
                                            ui.label(
                                                RichText::new(subtitle)
                                                    .size(11.5)
                                                    .color(theme::TEXT_MUTED)
                                                    .monospace(),
                                            );
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .small_button(
                                                        RichText::new("x").color(theme::TEXT_MUTED),
                                                    )
                                                    .on_hover_text("Delete connection")
                                                    .clicked()
                                                {
                                                    delete_id = Some(conn.id.clone());
                                                }
                                            },
                                        );
                                    });
                                });

                            let interact = ui.interact(
                                frame_resp.response.rect,
                                ui.id().with(("saved_conn", idx)),
                                egui::Sense::click(),
                            );
                            if interact.double_clicked() {
                                connect_idx = Some(idx);
                            } else if interact.clicked() {
                                load_idx = Some(idx);
                            }
                            ui.add_space(4.0);
                        }
                    });

                    if let Some(id) = delete_id {
                        action = Some(ConnectionAction::Delete(id));
                    } else if let Some(idx) = connect_idx {
                        let cfg = self.saved_connections[idx].clone();
                        self.load_into_form(&cfg);
                        action = Some(ConnectionAction::Connect(cfg));
                    } else if let Some(idx) = load_idx {
                        let cfg = self.saved_connections[idx].clone();
                        self.load_into_form(&cfg);
                    }
                }
            });

        // Main content: centered connection form card
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme::BG_BASE).inner_margin(24.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.set_max_width(540.0);
                        ui.add_space(12.0);

                        egui::Frame::none()
                            .fill(theme::BG_SURFACE)
                            .stroke(Stroke::new(1.0_f32, theme::BORDER))
                            .rounding(Rounding::same(6.0))
                            .inner_margin(egui::Margin::same(22.0))
                            .show(ui, |ui| {
                                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                                    self.show_form_card(ui, &mut action);
                                });
                            });
                    });
                });
            });

        action
    }

    fn show_form_card(&mut self, ui: &mut Ui, action: &mut Option<ConnectionAction>) {
        // Header
        ui.horizontal(|ui| {
            let title = if self.selected_id.is_some() {
                "Edit Connection"
            } else {
                "New Connection"
            };
            ui.heading(RichText::new(title).color(theme::TEXT_PRIMARY).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::badge(ui, "MySQL / MariaDB", theme::BG_SELECTED, theme::ACCENT);
            });
        });

        ui.add_space(10.0);

        // Inline status / error banner
        if let Some((is_error, msg)) = &self.status_banner {
            let (bg, border, fg) = if *is_error {
                (theme::BG_DELETED, theme::DANGER, Color32::from_rgb(255, 200, 198))
            } else {
                (theme::BG_INSERTED, theme::SUCCESS, Color32::from_rgb(180, 245, 215))
            };
            egui::Frame::none()
                .fill(bg)
                .stroke(Stroke::new(1.0_f32, border))
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(msg).color(fg));
                });
            ui.add_space(10.0);
        }

        let busy = self.is_busy();
        ui.add_enabled_ui(!busy, |ui| {
            // Connection Name
            field_label(ui, "Connection Name");
            ui.add(
                egui::TextEdit::singleline(&mut self.form.name)
                    .hint_text("e.g. Local MySQL")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);

            // Host & Port
            let total_w = ui.available_width();
            let port_w = 100.0;
            let host_w = (total_w - port_w - 10.0).max(120.0);

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    field_label(ui, "Host");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.host)
                            .hint_text("127.0.0.1")
                            .desired_width(host_w),
                    );
                });
                ui.vertical(|ui| {
                    field_label(ui, "Port");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.port_text)
                            .hint_text("3306")
                            .desired_width(port_w),
                    );
                });
            });
            ui.add_space(8.0);

            // Username & Password
            let half_w = ((total_w - 10.0) / 2.0).max(100.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    field_label(ui, "User");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.username)
                            .hint_text("root")
                            .desired_width(half_w),
                    );
                });
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        field_label(ui, "Password");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let toggle = if self.show_password { "Hide" } else { "Show" };
                            if ui.small_button(toggle).clicked() {
                                self.show_password = !self.show_password;
                            }
                        });
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.password)
                            .password(!self.show_password)
                            .hint_text("••••••••")
                            .desired_width(half_w),
                    );
                });
            });
            ui.add_space(8.0);

            // Default Database
            field_label(ui, "Default Database (optional)");
            ui.add(
                egui::TextEdit::singleline(&mut self.database_text)
                    .hint_text("Leave empty to browse all databases")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);

            // SSL & Timeout row
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.form.use_ssl, "Enable SSL / TLS");
                ui.add_space(20.0);
                ui.label(RichText::new("Timeout (s):").color(theme::TEXT_MUTED));
                ui.add(
                    egui::TextEdit::singleline(&mut self.timeout_text)
                        .desired_width(55.0),
                );
            });
        });

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(10.0);

        // Action buttons
        ui.horizontal(|ui| {
            if busy {
                ui.spinner();
                let label = if self.connecting {
                    "Connecting..."
                } else {
                    "Testing connection..."
                };
                ui.label(RichText::new(label).color(theme::ACCENT));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if theme::accent_button(ui, "Connect").clicked() {
                        match self.build_config() {
                            Ok(cfg) => {
                                self.status_banner = None;
                                *action = Some(ConnectionAction::Connect(cfg));
                            }
                            Err(e) => self.status_banner = Some((true, e)),
                        }
                    }

                    if ui.button("Save").clicked() {
                        match self.build_config() {
                            Ok(cfg) => {
                                self.selected_id = Some(cfg.id.clone());
                                self.form = cfg.clone();
                                *action = Some(ConnectionAction::Save(cfg));
                            }
                            Err(e) => self.status_banner = Some((true, e)),
                        }
                    }

                    if ui.button("Test Connection").clicked() {
                        match self.build_config() {
                            Ok(cfg) => {
                                self.status_banner = None;
                                *action = Some(ConnectionAction::Test(cfg));
                            }
                            Err(e) => self.status_banner = Some((true, e)),
                        }
                    }
                });
            });
        });
    }
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(theme::TEXT_MUTED));
}
