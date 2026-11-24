use crate::models::{QueryResult, SortSpec};
use crate::ui::data_grid::{
    compute_sorted_indices, next_sort_spec, render_readonly_result_grid, show_export_dialog,
    ExportDialog, ExportFormat,
};
use crate::ui::theme;
use egui::{Align, Color32, Layout, RichText, Rounding, Stroke, Ui};

/// Actions emitted by a SQL query editor tab.
#[derive(Debug, Clone)]
pub enum SqlEditorAction {
    Execute(String),
    Cancel(u64),
    Export { format: ExportFormat, path: String },
}

/// State for an individual SQL query editor tab.
pub struct SqlTabState {
    pub id: u64,
    pub title: String,
    pub sql: String,
    pub running_request_id: Option<u64>,
    pub result: Option<QueryResult>,
    pub sort: Option<SortSpec>,
    pub sorted_row_indices: Vec<usize>,
    pub error: Option<String>,
    pub export_modal: Option<ExportDialog>,
}

impl SqlTabState {
    pub fn new(id: u64, index: usize) -> Self {
        Self {
            id,
            title: format!("Query #{index}"),
            sql: "SELECT 1;".to_string(),
            running_request_id: None,
            result: None,
            sort: None,
            sorted_row_indices: Vec::new(),
            error: None,
            export_modal: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running_request_id.is_some()
    }

    /// Store a new query result and reset client-side sort permutation.
    pub fn set_result(&mut self, result: QueryResult) {
        self.running_request_id = None;
        self.error = None;
        self.sort = None;
        self.sorted_row_indices = (0..result.rows.len()).collect();
        self.result = Some(result);
    }

    /// Toggle client-side column sorting (`None -> Asc -> Desc -> None`) without cloning rows.
    pub fn toggle_sort(&mut self, column: String) {
        self.sort = next_sort_spec(self.sort.as_ref(), column);
        if let Some(res) = &self.result {
            self.sorted_row_indices = compute_sorted_indices(res, self.sort.as_ref());
        }
    }

    /// Render the SQL editor tab (toolbar + monospace editor + sortable result grid).
    pub fn show(&mut self, ui: &mut Ui) -> Option<SqlEditorAction> {
        let mut action: Option<SqlEditorAction> = None;

        // Export dialog modal if open
        if let Some((format, path)) =
            show_export_dialog(ui.ctx(), &mut self.export_modal, ("sql_export", self.id))
        {
            action = Some(SqlEditorAction::Export { format, path });
        }

        // Ctrl+Enter / Cmd+Enter shortcut to run query
        let run_shortcut = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        if run_shortcut && !self.is_running() && !self.sql.trim().is_empty() {
            action = Some(SqlEditorAction::Execute(self.sql.clone()));
        }

        // Top toolbar
        ui.horizontal(|ui| {
            if let Some(req_id) = self.running_request_id {
                if theme::danger_button(ui, "Cancel").clicked() {
                    action = Some(SqlEditorAction::Cancel(req_id));
                }
                ui.spinner();
                ui.label(RichText::new("Running query...").color(theme::ACCENT));
            } else {
                let can_run = !self.sql.trim().is_empty();
                ui.add_enabled_ui(can_run, |ui| {
                    if theme::accent_button(ui, "Run (Ctrl+Enter)").clicked() {
                        action = Some(SqlEditorAction::Execute(self.sql.clone()));
                    }
                });
            }

            // Execution stats badge
            if let Some(res) = &self.result {
                ui.separator();
                if !res.columns.is_empty() {
                    theme::badge(
                        ui,
                        &format!("{} rows", res.rows.len()),
                        theme::BG_SELECTED,
                        theme::ACCENT,
                    );
                } else if let Some(affected) = res.affected_rows {
                    theme::badge(
                        ui,
                        &format!("{affected} affected"),
                        theme::BG_INSERTED,
                        theme::SUCCESS,
                    );
                }
                theme::badge(
                    ui,
                    &format!("{} ms", res.execution_ms),
                    theme::BG_SURFACE,
                    theme::TEXT_MUTED,
                );
            }

            // Right side: Export CSV / JSON
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let has_rows = self
                    .result
                    .as_ref()
                    .is_some_and(|r| !r.columns.is_empty());
                ui.add_enabled_ui(has_rows, |ui| {
                    if ui.button("Export JSON").clicked() {
                        self.export_modal =
                            Some(ExportDialog::new(ExportFormat::Json, &self.title));
                    }
                    if ui.button("Export CSV").clicked() {
                        self.export_modal =
                            Some(ExportDialog::new(ExportFormat::Csv, &self.title));
                    }
                });
            });
        });

        ui.add_space(6.0);

        // Multi-line monospace SQL editor (~38% height, bounded between 120px and 320px)
        let editor_height = (ui.available_height() * 0.38).clamp(120.0, 320.0);
        egui::Frame::none()
            .fill(theme::BG_SIDEBAR)
            .stroke(Stroke::new(1.0_f32, theme::BORDER))
            .rounding(Rounding::same(4.0))
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("sql_editor_scroll", self.id))
                    .max_height(editor_height)
                    .min_scrolled_height(editor_height)
                    .show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), editor_height - 12.0],
                            egui::TextEdit::multiline(&mut self.sql)
                                .font(egui::TextStyle::Monospace)
                                .code_editor()
                                .hint_text("Write SQL here... Press Ctrl+Enter to run.")
                                .frame(false),
                        );
                    });
            });

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);

        // Result / error area
        let mut clicked_sort_col: Option<String> = None;
        if let Some(err) = &self.error {
            egui::Frame::none()
                .fill(theme::BG_DELETED)
                .stroke(Stroke::new(1.0_f32, theme::DANGER))
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(err)
                            .monospace()
                            .color(Color32::from_rgb(255, 200, 198)),
                    );
                });
        } else if let Some(res) = &self.result {
            if self.sorted_row_indices.len() != res.rows.len() {
                self.sorted_row_indices = compute_sorted_indices(res, self.sort.as_ref());
            }
            clicked_sort_col = render_readonly_result_grid(
                ui,
                res,
                &self.sorted_row_indices,
                self.sort.as_ref(),
                self.id,
            );
        } else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Run a query with Run or Ctrl+Enter to see results.")
                        .color(theme::TEXT_MUTED),
                );
            });
        }

        if let Some(col_name) = clicked_sort_col {
            self.toggle_sort(col_name);
        }

        action
    }
}
