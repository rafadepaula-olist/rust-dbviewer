use crate::models::{
    CellValue, ColumnInfo, FilterOperator, FilterSpec, PendingDelete, PendingEdit, PendingInsert,
    QueryResult, SortDirection, SortSpec, TableChanges,
};
use crate::ui::theme;
use egui::{Align, Color32, Layout, RichText, Rounding, Stroke, Ui};
use egui_extras::{Column, TableBuilder};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

pub const PAGE_SIZES: &[u64] = &[50, 100, 500, 1000];
const ROW_HEIGHT: f32 = 24.0;
const HEADER_HEIGHT: f32 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
}

impl ExportFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Json => "JSON",
        }
    }
}

/// Lightweight inline export path prompt (avoids blocking native dialog dependencies).
#[derive(Debug, Clone)]
pub struct ExportDialog {
    pub format: ExportFormat,
    pub path_input: String,
}

impl ExportDialog {
    pub fn new(format: ExportFormat, default_stem: &str) -> Self {
        let base_dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let safe_stem: String = default_stem
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let filename = format!("{}.{}", safe_stem, format.extension());
        let path_input = base_dir.join(filename).to_string_lossy().to_string();
        Self { format, path_input }
    }
}

/// Render an export path modal if open. Returns `Some((format, path))` on confirm.
pub fn show_export_dialog(
    ctx: &egui::Context,
    dialog: &mut Option<ExportDialog>,
    id_salt: impl std::hash::Hash,
) -> Option<(ExportFormat, String)> {
    let current = dialog.as_mut()?;
    let mut confirmed: Option<(ExportFormat, String)> = None;
    let mut close = false;

    egui::Window::new(format!("Export {}", current.format.label()))
        .id(egui::Id::new(id_salt))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.label(RichText::new("Output file path:").color(theme::TEXT_MUTED));
            let edit_resp = ui.add(
                egui::TextEdit::singleline(&mut current.path_input)
                    .desired_width(f32::INFINITY),
            );
            let enter_pressed =
                edit_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if (theme::accent_button(ui, "Export").clicked() || enter_pressed)
                        && !current.path_input.trim().is_empty()
                    {
                        confirmed = Some((current.format, current.path_input.trim().to_string()));
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        });

    if close {
        *dialog = None;
    }
    confirmed
}

/// A single editable filter row in the table filter bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterRow {
    pub column: String,
    pub operator: FilterOperator,
    pub value: String,
}

impl FilterRow {
    pub fn new(column: String) -> Self {
        Self {
            column,
            operator: FilterOperator::Eq,
            value: String::new(),
        }
    }
}

/// Active inline cell editor state.
#[derive(Debug, Clone)]
pub enum EditingCell {
    Existing {
        row_index: usize,
        col_index: usize,
        buffer: String,
        just_opened: bool,
    },
    Inserted {
        insert_index: usize,
        col_index: usize,
        buffer: String,
        just_opened: bool,
    },
}

/// Actions emitted by `DataGridState`.
#[derive(Debug, Clone)]
pub enum DataGridAction {
    Reload,
    ApplyChanges(TableChanges),
    Export { format: ExportFormat, path: String },
}

/// Compare two `CellValue`s with `Null` always sorted last regardless of direction.
pub fn compare_cell_values(a: &CellValue, b: &CellValue, direction: SortDirection) -> Ordering {
    match (a, b) {
        (CellValue::Null, CellValue::Null) => return Ordering::Equal,
        (CellValue::Null, _) => return Ordering::Greater,
        (_, CellValue::Null) => return Ordering::Less,
        _ => {}
    }

    let base = match (a, b) {
        (CellValue::Int(x), CellValue::Int(y)) => x.cmp(y),
        (CellValue::Float(x), CellValue::Float(y)) => x.total_cmp(y),
        (CellValue::Int(x), CellValue::Float(y)) => (*x as f64).total_cmp(y),
        (CellValue::Float(x), CellValue::Int(y)) => x.total_cmp(&(*y as f64)),
        (CellValue::Bool(x), CellValue::Bool(y)) => x.cmp(y),
        (CellValue::Text(x), CellValue::Text(y)) => x
            .to_lowercase()
            .cmp(&y.to_lowercase())
            .then_with(|| x.cmp(y)),
        (CellValue::Bytes(x), CellValue::Bytes(y)) => x.cmp(y),
        _ => a
            .to_display_string()
            .to_lowercase()
            .cmp(&b.to_display_string().to_lowercase()),
    };

    match direction {
        SortDirection::Asc => base,
        SortDirection::Desc => base.reverse(),
    }
}

/// Compute a sorted row-index permutation without cloning rows.
pub fn compute_sorted_indices(result: &QueryResult, sort: Option<&SortSpec>) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..result.rows.len()).collect();
    let Some(spec) = sort else {
        return indices;
    };
    let Some(col_idx) = result.columns.iter().position(|c| c.name == spec.column) else {
        return indices;
    };

    indices.sort_by(|&a, &b| {
        let va = result
            .rows
            .get(a)
            .and_then(|r| r.get(col_idx))
            .unwrap_or(&CellValue::Null);
        let vb = result
            .rows
            .get(b)
            .and_then(|r| r.get(col_idx))
            .unwrap_or(&CellValue::Null);
        compare_cell_values(va, vb, spec.direction)
    });
    indices
}

/// Cycle sort state on column header click: `None -> Asc -> Desc -> None`.
pub fn next_sort_spec(current: Option<&SortSpec>, clicked_column: String) -> Option<SortSpec> {
    match current {
        Some(prev) if prev.column == clicked_column => match prev.direction {
            SortDirection::Asc => Some(SortSpec {
                column: clicked_column,
                direction: SortDirection::Desc,
            }),
            SortDirection::Desc => None,
        },
        _ => Some(SortSpec {
            column: clicked_column,
            direction: SortDirection::Asc,
        }),
    }
}

/// State for a table browser tab with virtualized grid, sorting, multi-filter AND, pagination, and inline editing.
pub struct DataGridState {
    pub database: String,
    pub table: String,
    pub result: Option<QueryResult>,
    pub loading: bool,
    pub pending_request_id: Option<u64>,
    pub error: Option<String>,

    // Sorting & multi-AND filtering
    pub sort: Option<SortSpec>,
    pub filter_rows: Vec<FilterRow>,
    pub active_filters: Vec<FilterSpec>,

    // Pagination (1-based page)
    pub page: u64,
    pub page_size: u64,

    // Inline editing
    pub selected_row: Option<usize>,
    pub editing_cell: Option<EditingCell>,
    pub pending_edits: HashMap<(usize, usize), CellValue>,
    pub pending_deletes: HashSet<usize>,
    pub pending_inserts: Vec<Vec<CellValue>>,

    // Export modal
    pub export_modal: Option<ExportDialog>,
}

impl DataGridState {
    pub fn new(database: String, table: String) -> Self {
        Self {
            database,
            table,
            result: None,
            loading: false,
            pending_request_id: None,
            error: None,
            sort: None,
            filter_rows: vec![FilterRow::new(String::new())],
            active_filters: Vec::new(),
            page: 1,
            page_size: 100,
            selected_row: None,
            editing_cell: None,
            pending_edits: HashMap::new(),
            pending_deletes: HashSet::new(),
            pending_inserts: Vec::new(),
            export_modal: None,
        }
    }

    pub fn set_result(&mut self, page: u64, page_size: u64, result: QueryResult) {
        self.page = page.max(1);
        self.page_size = page_size.max(1);
        if let Some(first_col) = result.columns.first() {
            for row in &mut self.filter_rows {
                if row.column.is_empty() {
                    row.column = first_col.name.clone();
                }
            }
        }
        self.result = Some(result);
        self.loading = false;
        self.pending_request_id = None;
        self.error = None;
        self.discard_changes();
    }

    /// Collect all valid `FilterSpec`s from the current `filter_rows`.
    pub fn apply_filter_rows(&mut self) {
        let default_col = self
            .result
            .as_ref()
            .and_then(|r| r.columns.first())
            .map(|c| c.name.as_str())
            .unwrap_or("");

        self.active_filters = self
            .filter_rows
            .iter()
            .filter_map(|row| {
                let col = if row.column.is_empty() {
                    default_col
                } else {
                    row.column.as_str()
                };
                if col.is_empty() {
                    return None;
                }
                if row.operator.takes_value() && row.value.trim().is_empty() {
                    return None;
                }
                Some(FilterSpec {
                    column: col.to_string(),
                    operator: row.operator,
                    value: row.value.clone(),
                })
            })
            .collect();
        self.page = 1;
    }

    /// Clear all active filters and reset the filter bar to a single empty row.
    pub fn clear_filters(&mut self) {
        let default_col = self
            .result
            .as_ref()
            .and_then(|r| r.columns.first())
            .map(|c| c.name.clone())
            .unwrap_or_default();
        self.filter_rows = vec![FilterRow::new(default_col)];
        self.active_filters.clear();
        self.page = 1;
    }

    pub fn can_edit(&self) -> bool {
        self.result
            .as_ref()
            .is_some_and(|r| !r.primary_key_columns.is_empty())
    }

    pub fn pending_change_count(&self) -> usize {
        self.pending_edits.len() + self.pending_inserts.len() + self.pending_deletes.len()
    }

    pub fn has_pending_changes(&self) -> bool {
        self.pending_change_count() > 0
    }

    pub fn discard_changes(&mut self) {
        self.editing_cell = None;
        self.pending_edits.clear();
        self.pending_deletes.clear();
        self.pending_inserts.clear();
    }

    pub fn total_pages(&self) -> u64 {
        let total = self
            .result
            .as_ref()
            .and_then(|r| r.total_rows)
            .unwrap_or_else(|| self.result.as_ref().map_or(0, |r| r.rows.len() as u64));
        if total == 0 {
            1
        } else {
            total.div_ceil(self.page_size.max(1))
        }
    }

    /// Programmatically stage a cell edit (also used by tests).
    pub fn stage_edit(&mut self, row_index: usize, col_index: usize, new_value: CellValue) {
        let is_same_as_original = self
            .result
            .as_ref()
            .and_then(|r| r.rows.get(row_index))
            .and_then(|row| row.get(col_index))
            .is_some_and(|orig| orig == &new_value);

        if is_same_as_original {
            self.pending_edits.remove(&(row_index, col_index));
        } else {
            self.pending_edits.insert((row_index, col_index), new_value);
        }
    }

    /// Stage a new blank row for insertion.
    pub fn add_empty_row(&mut self) {
        if let Some(res) = &self.result {
            let row = vec![CellValue::Null; res.columns.len()];
            self.pending_inserts.push(row);
        }
    }

    /// Toggle deletion marker on an existing row index.
    pub fn toggle_delete_row(&mut self, row_index: usize) {
        if !self.pending_deletes.remove(&row_index) {
            self.pending_deletes.insert(row_index);
        }
    }

    /// Build a `TableChanges` payload from all staged edits, inserts, and deletes.
    pub fn build_table_changes(&self) -> Option<TableChanges> {
        let res = self.result.as_ref()?;
        if !self.has_pending_changes() || res.primary_key_columns.is_empty() {
            return None;
        }

        let mut edits: Vec<PendingEdit> = self
            .pending_edits
            .iter()
            .filter(|((row_idx, _), _)| !self.pending_deletes.contains(row_idx))
            .map(|(&(row_index, col_index), val)| PendingEdit {
                row_index,
                col_index,
                new_value: val.clone(),
            })
            .collect();
        edits.sort_by_key(|e| (e.row_index, e.col_index));

        let inserts: Vec<PendingInsert> = self
            .pending_inserts
            .iter()
            .map(|row| {
                let values = res
                    .columns
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, col)| {
                        let val = row.get(idx).cloned().unwrap_or(CellValue::Null);
                        if val.is_null()
                            && !col.nullable
                            && (col.default_value.is_some() || col.is_primary_key)
                        {
                            None
                        } else {
                            Some((col.name.clone(), val))
                        }
                    })
                    .collect();
                PendingInsert { values }
            })
            .collect();

        let mut deletes: Vec<PendingDelete> = self
            .pending_deletes
            .iter()
            .map(|&row_index| PendingDelete { row_index })
            .collect();
        deletes.sort_by_key(|d| d.row_index);

        let changes = TableChanges {
            database: self.database.clone(),
            table: self.table.clone(),
            columns: res.columns.clone(),
            primary_key_columns: res.primary_key_columns.clone(),
            original_rows: res.rows.clone(),
            edits,
            inserts,
            deletes,
        };

        if changes.is_empty() {
            None
        } else {
            Some(changes)
        }
    }

    /// Render the complete table tab: multi-row filter bar, action toolbar, virtualized grid, and pagination bar.
    pub fn show(&mut self, ui: &mut Ui, tab_id: u64) -> Option<DataGridAction> {
        let mut action: Option<DataGridAction> = None;

        // Export dialog modal if open
        if let Some((format, path)) =
            show_export_dialog(ui.ctx(), &mut self.export_modal, ("table_export", tab_id))
        {
            action = Some(DataGridAction::Export { format, path });
        }

        // Top multi-row AND filter bar & edit toolbar
        self.show_filter_bar(ui, tab_id, &mut action);
        ui.separator();
        self.show_edit_toolbar(ui, &mut action);
        ui.separator();

        // Error banner if present
        if let Some(err) = &self.error {
            egui::Frame::none()
                .fill(theme::BG_DELETED)
                .stroke(Stroke::new(1.0_f32, theme::DANGER))
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(err).color(Color32::from_rgb(255, 200, 198)));
                });
            ui.add_space(4.0);
        }

        // Reserve space for pagination bar at bottom
        let pagination_height = 34.0;
        let grid_height = (ui.available_height() - pagination_height - 8.0).max(120.0);

        // Main virtualized data grid
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), grid_height),
            Layout::top_down(Align::LEFT),
            |ui| {
                self.show_grid_body(ui, tab_id, &mut action);
            },
        );

        ui.separator();

        // Bottom pagination bar
        self.show_pagination_bar(ui, tab_id, &mut action);

        action
    }

    fn show_filter_bar(
        &mut self,
        ui: &mut Ui,
        tab_id: u64,
        action: &mut Option<DataGridAction>,
    ) {
        let col_names: Vec<String> = self
            .result
            .as_ref()
            .map(|r| r.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();

        if self.filter_rows.is_empty() {
            let default_col = col_names.first().cloned().unwrap_or_default();
            self.filter_rows.push(FilterRow::new(default_col));
        }

        let mut submit_filter = false;
        let mut clear_filter = false;
        let mut add_row = false;
        let mut remove_idx: Option<usize> = None;

        let row_count = self.filter_rows.len();
        let has_any_text = self
            .filter_rows
            .iter()
            .any(|r| !r.value.is_empty() || !r.operator.takes_value());

        for (idx, row) in self.filter_rows.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                // Prefix label: WHERE on row 0, AND on additional rows
                let prefix = if idx == 0 { "WHERE" } else { "AND" };
                let prefix_color = if idx == 0 {
                    theme::TEXT_MUTED
                } else {
                    theme::ACCENT
                };
                ui.add_sized(
                    [46.0, 22.0],
                    egui::Label::new(
                        RichText::new(prefix)
                            .monospace()
                            .strong()
                            .size(11.5)
                            .color(prefix_color),
                    ),
                );

                // Column selector
                let selected_col = if row.column.is_empty() {
                    col_names.first().map(String::as_str).unwrap_or("Column")
                } else {
                    row.column.as_str()
                };
                egui::ComboBox::from_id_salt(("filter_col", tab_id, idx))
                    .selected_text(selected_col)
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for name in &col_names {
                            if ui.selectable_label(row.column == *name, name).clicked() {
                                row.column = name.clone();
                            }
                        }
                    });

                // Operator selector
                egui::ComboBox::from_id_salt(("filter_op", tab_id, idx))
                    .selected_text(row.operator.as_sql())
                    .width(88.0)
                    .show_ui(ui, |ui| {
                        for &op in FilterOperator::ALL {
                            if ui.selectable_label(row.operator == op, op.as_sql()).clicked() {
                                row.operator = op;
                            }
                        }
                    });

                // Value input (only when operator takes a value)
                if row.operator.takes_value() {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut row.value)
                            .hint_text("Value... (Enter to apply)")
                            .desired_width(180.0),
                    );
                    if (resp.lost_focus() || resp.has_focus())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        submit_filter = true;
                    }
                }

                // Remove filter row button
                if row_count > 1
                    && ui
                        .small_button("-")
                        .on_hover_text("Remove filter condition")
                        .clicked()
                {
                    remove_idx = Some(idx);
                }

                // Action controls on the last filter row
                if idx + 1 == row_count {
                    if ui
                        .small_button("+")
                        .on_hover_text("Add AND filter condition")
                        .clicked()
                    {
                        add_row = true;
                    }

                    if theme::accent_button(ui, "Apply").clicked() {
                        submit_filter = true;
                    }

                    if (!self.active_filters.is_empty() || has_any_text)
                        && ui.button("Clear").clicked()
                    {
                        clear_filter = true;
                    }
                }

                // Right-aligned Refresh + spinner on first row
                if idx == 0 {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Refresh").clicked() {
                            *action = Some(DataGridAction::Reload);
                        }
                        if self.loading {
                            ui.spinner();
                        }
                        if !self.active_filters.is_empty() {
                            theme::badge(
                                ui,
                                &format!("{} filter(s)", self.active_filters.len()),
                                theme::BG_SELECTED,
                                theme::ACCENT,
                            );
                        }
                    });
                }
            });
        }

        if let Some(idx) = remove_idx {
            self.filter_rows.remove(idx);
            self.apply_filter_rows();
            *action = Some(DataGridAction::Reload);
        } else if add_row {
            let default_col = col_names.first().cloned().unwrap_or_default();
            self.filter_rows.push(FilterRow::new(default_col));
        } else if clear_filter {
            self.clear_filters();
            *action = Some(DataGridAction::Reload);
        } else if submit_filter {
            self.apply_filter_rows();
            *action = Some(DataGridAction::Reload);
        }
    }

    fn show_edit_toolbar(&mut self, ui: &mut Ui, action: &mut Option<DataGridAction>) {
        let can_edit = self.can_edit();
        let change_count = self.pending_change_count();
        let has_result = self.result.is_some();

        ui.horizontal(|ui| {
            ui.add_enabled_ui(can_edit && !self.loading, |ui| {
                if ui
                    .button("+ Add Row")
                    .on_disabled_hover_text("Table requires a primary key for inline editing")
                    .clicked()
                {
                    self.add_empty_row();
                }

                let del_label = match self.selected_row {
                    Some(idx) if self.pending_deletes.contains(&idx) => "Unmark Delete",
                    _ => "Mark Row Deleted",
                };
                if ui
                    .add_enabled(self.selected_row.is_some(), egui::Button::new(del_label))
                    .on_hover_text("Select a row first to mark it for deletion")
                    .clicked()
                {
                    if let Some(row_idx) = self.selected_row {
                        self.toggle_delete_row(row_idx);
                    }
                }
            });

            if change_count > 0 {
                ui.separator();
                let apply_label = format!("Apply Changes ({change_count})");
                if theme::accent_button(ui, &apply_label).clicked() {
                    self.commit_active_editor();
                    if let Some(changes) = self.build_table_changes() {
                        *action = Some(DataGridAction::ApplyChanges(changes));
                    }
                }
                if ui.button("Discard").clicked() {
                    self.discard_changes();
                }
            } else if !can_edit && has_result {
                ui.label(
                    RichText::new("Read-only (no primary key)")
                        .size(11.5)
                        .color(theme::TEXT_MUTED),
                );
            }

            // Right side: Export CSV & Export JSON
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(has_result, |ui| {
                    if ui.button("Export JSON").clicked() {
                        self.export_modal =
                            Some(ExportDialog::new(ExportFormat::Json, &self.table));
                    }
                    if ui.button("Export CSV").clicked() {
                        self.export_modal =
                            Some(ExportDialog::new(ExportFormat::Csv, &self.table));
                    }
                });
            });
        });
    }

    fn commit_active_editor(&mut self) {
        let Some(editing) = self.editing_cell.take() else {
            return;
        };
        let columns = match &self.result {
            Some(r) => &r.columns,
            None => return,
        };
        match editing {
            EditingCell::Existing {
                row_index,
                col_index,
                buffer,
                ..
            } => {
                let col_type = columns
                    .get(col_index)
                    .map(|c| c.data_type.as_str())
                    .unwrap_or("");
                let parsed = parse_edited_value(&buffer, col_type);
                self.stage_edit(row_index, col_index, parsed);
            }
            EditingCell::Inserted {
                insert_index,
                col_index,
                buffer,
                ..
            } => {
                let col_type = columns
                    .get(col_index)
                    .map(|c| c.data_type.as_str())
                    .unwrap_or("");
                let parsed = parse_edited_value(&buffer, col_type);
                if let Some(row) = self.pending_inserts.get_mut(insert_index) {
                    if let Some(cell) = row.get_mut(col_index) {
                        *cell = parsed;
                    }
                }
            }
        }
    }

    fn show_grid_body(
        &mut self,
        ui: &mut Ui,
        tab_id: u64,
        action: &mut Option<DataGridAction>,
    ) {
        let Some(result) = self.result.as_ref() else {
            ui.centered_and_justified(|ui| {
                if self.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading table data...");
                    });
                } else {
                    ui.label(RichText::new("No data loaded.").color(theme::TEXT_MUTED));
                }
            });
            return;
        };

        if result.columns.is_empty() {
            ui.label(RichText::new("Table has no columns.").color(theme::TEXT_MUTED));
            return;
        }

        let num_cols = result.columns.len();
        let existing_rows = result.rows.len();
        let insert_rows = self.pending_inserts.len();
        let total_display_rows = existing_rows + insert_rows;
        let can_edit = !result.primary_key_columns.is_empty();

        let mut clicked_sort_col: Option<String> = None;
        let mut start_edit: Option<EditingCell> = None;
        let mut finish_edit = false;
        let mut cancel_edit = false;
        let mut set_edit_null = false;
        let mut new_selected_row: Option<usize> = None;

        // Horizontal scroll wrapper so wide tables scroll cleanly
        egui::ScrollArea::horizontal()
            .id_salt(("table_hscroll", tab_id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut builder = TableBuilder::new(ui)
                    .id_salt(("data_grid_table", tab_id))
                    .striped(true)
                    .resizable(true)
                    .cell_layout(Layout::left_to_right(Align::Center));

                // Row number / status column
                builder = builder.column(Column::initial(42.0).at_least(36.0));
                for _ in 0..num_cols {
                    builder = builder.column(Column::initial(150.0).at_least(70.0).clip(true));
                }

                builder
                    .header(HEADER_HEIGHT, |mut header| {
                        header.col(|ui| {
                            ui.label(RichText::new("#").color(theme::TEXT_MUTED).size(11.5));
                        });
                        for col in &result.columns {
                            header.col(|ui| {
                                if render_sortable_header_cell(ui, col, self.sort.as_ref()) {
                                    clicked_sort_col = Some(col.name.clone());
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(ROW_HEIGHT, total_display_rows, |mut row| {
                            let display_idx = row.index();
                            let is_inserted = display_idx >= existing_rows;
                            let is_deleted =
                                !is_inserted && self.pending_deletes.contains(&display_idx);
                            let is_selected =
                                !is_inserted && self.selected_row == Some(display_idx);

                            // Row index column
                            row.col(|ui| {
                                if is_inserted {
                                    ui.painter()
                                        .rect_filled(ui.max_rect(), 0.0, theme::BG_INSERTED);
                                    ui.label(RichText::new("+").color(theme::SUCCESS).strong());
                                } else {
                                    if is_deleted {
                                        ui.painter().rect_filled(
                                            ui.max_rect(),
                                            0.0,
                                            theme::BG_DELETED,
                                        );
                                    } else if is_selected {
                                        ui.painter().rect_filled(
                                            ui.max_rect(),
                                            0.0,
                                            theme::BG_SELECTED,
                                        );
                                    }
                                    let row_num = (self.page.saturating_sub(1)) * self.page_size
                                        + (display_idx as u64)
                                        + 1;
                                    let resp = ui.add(
                                        egui::Button::new(
                                            RichText::new(row_num.to_string())
                                                .size(11.0)
                                                .color(if is_deleted {
                                                    theme::DANGER
                                                } else {
                                                    theme::TEXT_MUTED
                                                })
                                                .monospace(),
                                        )
                                        .frame(false),
                                    );
                                    if resp.clicked() {
                                        new_selected_row = Some(display_idx);
                                    }
                                }
                            });

                            // Data columns
                            for col_idx in 0..num_cols {
                                row.col(|ui| {
                                    if is_inserted {
                                        let ins_idx = display_idx - existing_rows;
                                        ui.painter().rect_filled(
                                            ui.max_rect(),
                                            0.0,
                                            theme::BG_INSERTED,
                                        );

                                        let is_editing = matches!(
                                            self.editing_cell,
                                            Some(EditingCell::Inserted { insert_index, col_index, .. })
                                                if insert_index == ins_idx && col_index == col_idx
                                        );

                                        if is_editing {
                                            if let Some(EditingCell::Inserted {
                                                buffer,
                                                just_opened,
                                                ..
                                            }) = self.editing_cell.as_mut()
                                            {
                                                render_inline_editor(
                                                    ui,
                                                    buffer,
                                                    just_opened,
                                                    &mut finish_edit,
                                                    &mut cancel_edit,
                                                    &mut set_edit_null,
                                                );
                                            }
                                        } else {
                                            let val = self
                                                .pending_inserts
                                                .get(ins_idx)
                                                .and_then(|r| r.get(col_idx))
                                                .unwrap_or(&CellValue::Null);
                                            let resp = render_cell_value(ui, val, false);
                                            if can_edit && resp.double_clicked() {
                                                start_edit = Some(EditingCell::Inserted {
                                                    insert_index: ins_idx,
                                                    col_index: col_idx,
                                                    buffer: val.to_edit_string(),
                                                    just_opened: true,
                                                });
                                            }
                                        }
                                    } else {
                                        let edited_val =
                                            self.pending_edits.get(&(display_idx, col_idx));
                                        let is_edited = edited_val.is_some();

                                        if is_deleted {
                                            ui.painter().rect_filled(
                                                ui.max_rect(),
                                                0.0,
                                                theme::BG_DELETED,
                                            );
                                        } else if is_edited {
                                            ui.painter().rect_filled(
                                                ui.max_rect(),
                                                0.0,
                                                theme::BG_EDITED,
                                            );
                                        } else if is_selected {
                                            ui.painter().rect_filled(
                                                ui.max_rect(),
                                                0.0,
                                                theme::BG_SELECTED,
                                            );
                                        }

                                        let is_editing = matches!(
                                            self.editing_cell,
                                            Some(EditingCell::Existing { row_index, col_index, .. })
                                                if row_index == display_idx && col_index == col_idx
                                        );

                                        if is_editing {
                                            if let Some(EditingCell::Existing {
                                                buffer,
                                                just_opened,
                                                ..
                                            }) = self.editing_cell.as_mut()
                                            {
                                                render_inline_editor(
                                                    ui,
                                                    buffer,
                                                    just_opened,
                                                    &mut finish_edit,
                                                    &mut cancel_edit,
                                                    &mut set_edit_null,
                                                );
                                            }
                                        } else {
                                            let val = edited_val.unwrap_or_else(|| {
                                                result
                                                    .rows
                                                    .get(display_idx)
                                                    .and_then(|r| r.get(col_idx))
                                                    .unwrap_or(&CellValue::Null)
                                            });
                                            let resp = render_cell_value(ui, val, is_deleted);
                                            if resp.clicked() {
                                                new_selected_row = Some(display_idx);
                                            }
                                            if can_edit && !is_deleted && resp.double_clicked() {
                                                start_edit = Some(EditingCell::Existing {
                                                    row_index: display_idx,
                                                    col_index: col_idx,
                                                    buffer: val.to_edit_string(),
                                                    just_opened: true,
                                                });
                                            }
                                        }
                                    }
                                });
                            }
                        });
                    });
            });

        // Apply deferred UI mutations
        if let Some(row_idx) = new_selected_row {
            self.selected_row = Some(row_idx);
        }
        if cancel_edit {
            self.editing_cell = None;
        } else if set_edit_null {
            if let Some(editing) = self.editing_cell.take() {
                match editing {
                    EditingCell::Existing {
                        row_index,
                        col_index,
                        ..
                    } => self.stage_edit(row_index, col_index, CellValue::Null),
                    EditingCell::Inserted {
                        insert_index,
                        col_index,
                        ..
                    } => {
                        if let Some(r) = self.pending_inserts.get_mut(insert_index) {
                            if let Some(c) = r.get_mut(col_index) {
                                *c = CellValue::Null;
                            }
                        }
                    }
                }
            }
        } else if finish_edit {
            self.commit_active_editor();
        }
        if let Some(next_edit) = start_edit {
            self.commit_active_editor();
            self.editing_cell = Some(next_edit);
        }

        // Handle column header sort click
        if let Some(col_name) = clicked_sort_col {
            self.sort = next_sort_spec(self.sort.as_ref(), col_name);
            self.page = 1;
            *action = Some(DataGridAction::Reload);
        }
    }

    fn show_pagination_bar(
        &mut self,
        ui: &mut Ui,
        tab_id: u64,
        action: &mut Option<DataGridAction>,
    ) {
        let total_pages = self.total_pages();
        let total_rows = self.result.as_ref().and_then(|r| r.total_rows);
        let exec_ms = self.result.as_ref().map(|r| r.execution_ms);

        ui.horizontal(|ui| {
            // Prev / Page N of M / Next
            if ui
                .add_enabled(self.page > 1 && !self.loading, egui::Button::new("< Prev"))
                .clicked()
            {
                self.page = self.page.saturating_sub(1).max(1);
                *action = Some(DataGridAction::Reload);
            }

            ui.label(
                RichText::new(format!("Page {} of {}", self.page, total_pages))
                    .monospace()
                    .size(12.5),
            );

            if ui
                .add_enabled(
                    self.page < total_pages && !self.loading,
                    egui::Button::new("Next >"),
                )
                .clicked()
            {
                self.page += 1;
                *action = Some(DataGridAction::Reload);
            }

            ui.separator();

            // Page size selector
            ui.label(RichText::new("Rows:").color(theme::TEXT_MUTED).size(12.0));
            egui::ComboBox::from_id_salt(("page_size", tab_id))
                .selected_text(self.page_size.to_string())
                .width(64.0)
                .show_ui(ui, |ui| {
                    for &size in PAGE_SIZES {
                        if ui
                            .selectable_label(self.page_size == size, size.to_string())
                            .clicked()
                            && self.page_size != size
                        {
                            self.page_size = size;
                            self.page = 1;
                            *action = Some(DataGridAction::Reload);
                        }
                    }
                });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(ms) = exec_ms {
                    ui.label(
                        RichText::new(format!("{ms} ms"))
                            .monospace()
                            .size(11.5)
                            .color(theme::TEXT_MUTED),
                    );
                    ui.separator();
                }
                if let Some(total) = total_rows {
                    ui.label(
                        RichText::new(format!("{total} total rows"))
                            .monospace()
                            .size(12.0)
                            .color(theme::TEXT_PRIMARY),
                    );
                }
            });
        });
    }
}

/// Render a clickable sortable column header with ASCII sort indicator (`^` / `v` / `<>`),
/// PK badge, and muted data type. Returns `true` if clicked.
fn render_sortable_header_cell(
    ui: &mut Ui,
    col: &ColumnInfo,
    active_sort: Option<&SortSpec>,
) -> bool {
    let sort_dir = active_sort
        .filter(|s| s.column == col.name)
        .map(|s| s.direction);

    let (arrow, arrow_color) = match sort_dir {
        Some(SortDirection::Asc) => ("^", theme::ACCENT),
        Some(SortDirection::Desc) => ("v", theme::ACCENT),
        None => ("<>", theme::TEXT_NULL),
    };

    let name_color = if sort_dir.is_some() {
        theme::ACCENT
    } else {
        theme::TEXT_PRIMARY
    };

    let mut label = if col.is_primary_key {
        format!("[PK] {}", col.name)
    } else {
        col.name.clone()
    };
    if !col.data_type.is_empty() {
        label.push_str(&format!(" ({})", col.data_type));
    }

    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let btn = ui
                .add(
                    egui::Button::new(
                        RichText::new(&col.name)
                            .strong()
                            .size(12.5)
                            .color(name_color),
                    )
                    .frame(false),
                )
                .on_hover_text(format!("{label}\nClick to sort"));

            if col.is_primary_key {
                theme::badge(ui, "PK", theme::BG_SELECTED, theme::ACCENT);
            }

            if !col.data_type.is_empty() {
                ui.label(
                    RichText::new(&col.data_type)
                        .monospace()
                        .size(10.5)
                        .color(theme::TEXT_MUTED),
                );
            }

            let arrow_btn = ui
                .add(
                    egui::Button::new(
                        RichText::new(arrow)
                            .monospace()
                            .strong()
                            .size(11.0)
                            .color(arrow_color),
                    )
                    .frame(false),
                )
                .on_hover_text("Click to sort");

            btn.clicked() || arrow_btn.clicked()
        })
        .inner;

    resp
}

/// Reusable virtualized read-only result grid for SQL query tabs with client-side column sorting.
/// Returns `Some(column_name)` when a column header is clicked.
pub fn render_readonly_result_grid(
    ui: &mut Ui,
    result: &QueryResult,
    row_indices: &[usize],
    sort: Option<&SortSpec>,
    id_salt: impl std::hash::Hash + Copy,
) -> Option<String> {
    if result.columns.is_empty() {
        if let Some(affected) = result.affected_rows {
            ui.label(
                RichText::new(format!(
                    "Query executed successfully. Affected rows: {affected} ({} ms)",
                    result.execution_ms
                ))
                .color(theme::SUCCESS),
            );
        } else {
            ui.label(
                RichText::new(format!(
                    "Query executed successfully ({} ms). No rows returned.",
                    result.execution_ms
                ))
                .color(theme::TEXT_MUTED),
            );
        }
        return None;
    }

    let num_cols = result.columns.len();
    let num_rows = row_indices.len();
    let mut clicked_col: Option<String> = None;

    egui::ScrollArea::horizontal()
        .id_salt(("sql_hscroll", id_salt))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut builder = TableBuilder::new(ui)
                .id_salt(("sql_result_table", id_salt))
                .striped(true)
                .resizable(true)
                .cell_layout(Layout::left_to_right(Align::Center));

            builder = builder.column(Column::initial(42.0).at_least(36.0));
            for _ in 0..num_cols {
                builder = builder.column(Column::initial(150.0).at_least(70.0).clip(true));
            }

            builder
                .header(HEADER_HEIGHT, |mut header| {
                    header.col(|ui| {
                        ui.label(RichText::new("#").color(theme::TEXT_MUTED).size(11.5));
                    });
                    for col in &result.columns {
                        header.col(|ui| {
                            if render_sortable_header_cell(ui, col, sort) {
                                clicked_col = Some(col.name.clone());
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_HEIGHT, num_rows, |mut row| {
                        let display_idx = row.index();
                        let actual_row_idx = row_indices[display_idx];
                        row.col(|ui| {
                            ui.label(
                                RichText::new((display_idx + 1).to_string())
                                    .size(11.0)
                                    .color(theme::TEXT_MUTED)
                                    .monospace(),
                            );
                        });
                        for col_idx in 0..num_cols {
                            row.col(|ui| {
                                let val = result
                                    .rows
                                    .get(actual_row_idx)
                                    .and_then(|r| r.get(col_idx))
                                    .unwrap_or(&CellValue::Null);
                                render_cell_value(ui, val, false);
                            });
                        }
                    });
                });
        });

    clicked_col
}

fn render_cell_value(ui: &mut Ui, val: &CellValue, strikethrough: bool) -> egui::Response {
    let text = match val {
        CellValue::Null => {
            let mut rt = RichText::new("NULL")
                .italics()
                .monospace()
                .size(12.0)
                .color(theme::TEXT_NULL);
            if strikethrough {
                rt = rt.strikethrough();
            }
            rt
        }
        CellValue::Bool(b) => {
            let mut rt = RichText::new(if *b { "true" } else { "false" })
                .monospace()
                .size(12.5)
                .color(theme::INFO);
            if strikethrough {
                rt = rt.strikethrough();
            }
            rt
        }
        CellValue::Int(i) => {
            let mut rt = RichText::new(i.to_string())
                .monospace()
                .size(12.5)
                .color(Color32::from_rgb(165, 224, 175));
            if strikethrough {
                rt = rt.strikethrough();
            }
            rt
        }
        CellValue::Float(f) => {
            let mut rt = RichText::new(f.to_string())
                .monospace()
                .size(12.5)
                .color(Color32::from_rgb(165, 224, 175));
            if strikethrough {
                rt = rt.strikethrough();
            }
            rt
        }
        CellValue::Text(s) => {
            let display = if s.contains('\n') {
                s.replace('\n', " \\n ")
            } else {
                s.clone()
            };
            let mut rt = RichText::new(display)
                .monospace()
                .size(12.5)
                .color(theme::TEXT_PRIMARY);
            if strikethrough {
                rt = rt.strikethrough();
            }
            rt
        }
        CellValue::Bytes(_) => {
            let mut rt = RichText::new(val.to_display_string())
                .monospace()
                .size(12.0)
                .color(theme::WARNING);
            if strikethrough {
                rt = rt.strikethrough();
            }
            rt
        }
    };

    ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click()))
}

fn render_inline_editor(
    ui: &mut Ui,
    buffer: &mut String,
    just_opened: &mut bool,
    finish: &mut bool,
    cancel: &mut bool,
    set_null: &mut bool,
) {
    let w = (ui.available_width() - 48.0).max(50.0);
    let resp = ui.add(
        egui::TextEdit::singleline(buffer)
            .font(egui::TextStyle::Monospace)
            .desired_width(w),
    );
    if *just_opened {
        resp.request_focus();
        *just_opened = false;
    }

    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        *cancel = true;
    } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        *finish = true;
    }

    if ui
        .small_button("NULL")
        .on_hover_text("Set cell to NULL")
        .clicked()
    {
        *set_null = true;
    }
}

/// Parse an inline-edited text buffer into a typed `CellValue` based on column data type.
pub fn parse_edited_value(input: &str, data_type: &str) -> CellValue {
    let upper = data_type.to_uppercase();
    let trimmed = input.trim();

    if upper.contains("BOOL") || upper == "TINYINT(1)" {
        match trimmed.to_lowercase().as_str() {
            "true" | "1" => return CellValue::Bool(true),
            "false" | "0" => return CellValue::Bool(false),
            _ => {}
        }
    }
    if upper.contains("INT") {
        if let Ok(v) = trimmed.parse::<i64>() {
            return CellValue::Int(v);
        }
    }
    if upper.contains("FLOAT") || upper.contains("DOUBLE") || upper.contains("REAL") {
        if let Ok(v) = trimmed.parse::<f64>() {
            return CellValue::Float(v);
        }
    }
    CellValue::Text(input.to_string())
}

/// Helper to check whether a column list contains any primary key.
pub fn has_primary_key(columns: &[ColumnInfo]) -> bool {
    columns.iter().any(|c| c.is_primary_key)
}
