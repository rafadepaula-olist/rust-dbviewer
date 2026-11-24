use crate::models::{ColumnInfo, DatabaseInfo, TableInfo, TableKind};
use crate::ui::theme;
use egui::{RichText, Ui};
use std::collections::{HashMap, HashSet};

/// Actions emitted by the connected left sidebar.
#[derive(Debug, Clone)]
pub enum SidebarAction {
    SwitchDatabase(String),
    OpenTable(String),
    LoadColumns(String),
    Refresh,
    Disconnect,
    NewQuery,
}

/// Filter tables by `query` (case-insensitive) and rank exact matches first,
/// followed by prefix matches, then substring matches, then alphabetical order.
pub fn filter_and_sort_tables<'a>(tables: &'a [TableInfo], query: &str) -> Vec<&'a TableInfo> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return tables.iter().collect();
    }

    let mut ranked: Vec<(u8, String, &'a TableInfo)> = tables
        .iter()
        .filter_map(|t| {
            let lower = t.name.to_lowercase();
            let rank = if lower == needle {
                0
            } else if lower.starts_with(&needle) {
                1
            } else if lower.contains(&needle) {
                2
            } else {
                return None;
            };
            Some((rank, lower, t))
        })
        .collect();

    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    ranked.into_iter().map(|(_, _, t)| t).collect()
}

/// UI state for the connected database explorer sidebar.
#[derive(Default)]
pub struct SidebarState {
    pub search_query: String,
    pub expanded_tables: HashSet<String>,
    pub loading_columns: HashSet<String>,
}

impl SidebarState {
    pub fn reset(&mut self) {
        self.search_query.clear();
        self.expanded_tables.clear();
        self.loading_columns.clear();
    }

    /// Render the left sidebar for a connected session.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        connection_name: &str,
        databases: &[DatabaseInfo],
        current_database: Option<&str>,
        tables: &[TableInfo],
        columns_cache: &HashMap<(String, String), Vec<ColumnInfo>>,
        loading_tables: bool,
    ) -> Option<SidebarAction> {
        let mut action: Option<SidebarAction> = None;

        // Top connection header
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(connection_name)
                    .strong()
                    .color(theme::ACCENT)
                    .size(14.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("Disconnect")
                    .on_hover_text("Disconnect and return to connection manager")
                    .clicked()
                {
                    action = Some(SidebarAction::Disconnect);
                }
                if ui
                    .small_button("Refresh")
                    .on_hover_text("Refresh databases and tables")
                    .clicked()
                {
                    action = Some(SidebarAction::Refresh);
                }
            });
        });

        ui.add_space(6.0);

        // Database selector dropdown
        ui.label(RichText::new("Database").size(11.5).color(theme::TEXT_MUTED));
        let selected_label = current_database.unwrap_or("Select database...");
        egui::ComboBox::from_id_salt("sidebar_db_combo")
            .selected_text(selected_label)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for db in databases {
                    let selected = current_database == Some(db.name.as_str());
                    if ui.selectable_label(selected, &db.name).clicked() && !selected {
                        action = Some(SidebarAction::SwitchDatabase(db.name.clone()));
                    }
                }
            });

        ui.add_space(6.0);

        // Search / filter input + New Query quick button
        ui.horizontal(|ui| {
            let search_w = (ui.available_width() - 62.0).max(80.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .hint_text("Filter tables...")
                    .desired_width(search_w),
            );
            if ui
                .button("+ SQL")
                .on_hover_text("Open new SQL query tab")
                .clicked()
            {
                action = Some(SidebarAction::NewQuery);
            }
        });

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);

        let filtered_tables = filter_and_sort_tables(tables, &self.search_query);
        let is_searching = !self.search_query.trim().is_empty();

        // Tables & Views header with filtered/total count
        ui.horizontal(|ui| {
            let header_text = if is_searching {
                format!(
                    "TABLES & VIEWS ({} / {})",
                    filtered_tables.len(),
                    tables.len()
                )
            } else {
                format!("TABLES & VIEWS ({})", tables.len())
            };
            ui.label(
                RichText::new(header_text)
                    .size(11.0)
                    .color(theme::TEXT_MUTED)
                    .strong(),
            );
            if loading_tables {
                ui.spinner();
            }
        });
        ui.add_space(4.0);

        if current_database.is_none() {
            ui.label(
                RichText::new("Select a database above to browse tables.")
                    .color(theme::TEXT_MUTED)
                    .size(12.0),
            );
            return action;
        }

        let db_name = current_database.unwrap_or_default();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for table in &filtered_tables {
                    let is_expanded = self.expanded_tables.contains(&table.name);
                    let arrow = if is_expanded { "v" } else { ">" };

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;

                        // Expand/collapse arrow (ASCII guaranteed to render cleanly)
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(arrow)
                                        .monospace()
                                        .size(11.5)
                                        .color(theme::TEXT_MUTED),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            if is_expanded {
                                self.expanded_tables.remove(&table.name);
                            } else {
                                self.expanded_tables.insert(table.name.clone());
                                let key = (db_name.to_string(), table.name.clone());
                                if !columns_cache.contains_key(&key) {
                                    self.loading_columns.insert(table.name.clone());
                                    action = Some(SidebarAction::LoadColumns(table.name.clone()));
                                }
                            }
                        }

                        // Kind tag (Table vs View)
                        match table.kind {
                            TableKind::Table => {
                                ui.label(
                                    RichText::new("T")
                                        .monospace()
                                        .strong()
                                        .color(theme::ACCENT)
                                        .size(11.0),
                                );
                            }
                            TableKind::View => {
                                ui.label(
                                    RichText::new("V")
                                        .monospace()
                                        .strong()
                                        .color(theme::INFO)
                                        .size(11.0),
                                );
                            }
                        }

                        // Clickable table name
                        let label_resp = ui.add(
                            egui::Button::new(
                                RichText::new(&table.name)
                                    .color(theme::TEXT_PRIMARY)
                                    .size(13.0),
                            )
                            .frame(false),
                        );
                        if label_resp.clicked() {
                            action = Some(SidebarAction::OpenTable(table.name.clone()));
                        }

                        if table.kind == TableKind::View {
                            theme::badge(
                                ui,
                                "VIEW",
                                egui::Color32::from_rgb(20, 48, 70),
                                theme::INFO,
                            );
                        }
                    });

                    // Expanded column list
                    if is_expanded {
                        let key = (db_name.to_string(), table.name.clone());
                        ui.indent(ui.id().with(("cols", &table.name)), |ui| {
                            if let Some(cols) = columns_cache.get(&key) {
                                for col in cols {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 5.0;
                                        if col.is_primary_key {
                                            theme::badge(
                                                ui,
                                                "PK",
                                                theme::BG_SELECTED,
                                                theme::ACCENT,
                                            );
                                        }
                                        ui.label(
                                            RichText::new(&col.name)
                                                .monospace()
                                                .size(12.0)
                                                .color(theme::TEXT_PRIMARY),
                                        );
                                        let mut dtype = col.data_type.clone();
                                        if col.nullable {
                                            dtype.push('?');
                                        }
                                        ui.label(
                                            RichText::new(dtype)
                                                .monospace()
                                                .size(11.0)
                                                .color(theme::TEXT_MUTED),
                                        );
                                    });
                                }
                            } else if self.loading_columns.contains(&table.name) {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(
                                        RichText::new("Loading columns...")
                                            .size(11.5)
                                            .color(theme::TEXT_MUTED),
                                    );
                                });
                            } else {
                                ui.label(
                                    RichText::new("No columns loaded")
                                        .size(11.5)
                                        .color(theme::TEXT_MUTED),
                                );
                            }
                        });
                        ui.add_space(2.0);
                    }
                }

                if filtered_tables.is_empty() && !loading_tables {
                    ui.label(
                        RichText::new(if tables.is_empty() {
                            "No tables in this database."
                        } else {
                            "No matching tables."
                        })
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                    );
                }
            });

        action
    }
}
