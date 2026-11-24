pub mod connection;
pub mod data_grid;
pub mod sidebar;
pub mod sql_editor;
pub mod theme;

use crate::export::{export_csv_file, export_json_file};
use crate::models::{
    ColumnInfo, ConnectionConfig, DatabaseInfo, DbCommand, DbEvent, QueryResult, TableInfo,
};
use crate::storage::{load_connections, save_connections};
use crate::ui::connection::{ConnectionAction, ConnectionScreenState};
use crate::ui::data_grid::{DataGridAction, DataGridState, ExportFormat};
use crate::ui::sidebar::{SidebarAction, SidebarState};
use crate::ui::sql_editor::{SqlEditorAction, SqlTabState};
use egui::{Align, Color32, Layout, RichText, Rounding, Stroke};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::Receiver as StdReceiver;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

const TOAST_DURATION_SECS: u64 = 5;

/// Active workspace tab (either a browsed table or a SQL query editor).
pub enum Tab {
    Table { id: u64, state: Box<DataGridState> },
    Sql(Box<SqlTabState>),
}

impl Tab {
    pub fn id(&self) -> u64 {
        match self {
            Tab::Table { id, .. } => *id,
            Tab::Sql(s) => s.id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Tab::Table { state, .. } => &state.table,
            Tab::Sql(s) => &s.title,
        }
    }

    pub fn is_dirty(&self) -> bool {
        match self {
            Tab::Table { state, .. } => state.has_pending_changes(),
            Tab::Sql(_) => false,
        }
    }

    pub fn is_busy(&self) -> bool {
        match self {
            Tab::Table { state, .. } => state.loading,
            Tab::Sql(s) => s.is_running(),
        }
    }
}

/// Ephemeral status/toast notification shown in the bottom status bar.
#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub is_error: bool,
    pub created_at: Instant,
}

/// Top-level eframe application for the native MySQL/MariaDB viewer.
pub struct DbViewerApp {
    cmd_tx: UnboundedSender<DbCommand>,
    event_rx: StdReceiver<DbEvent>,

    // Connection screen & session state
    pub connection_screen: ConnectionScreenState,
    pub active_connection: Option<ConnectionConfig>,
    pub server_version: String,

    // Connected workspace metadata
    pub databases: Vec<DatabaseInfo>,
    pub current_database: Option<String>,
    pub tables: Vec<TableInfo>,
    pub columns_cache: HashMap<(String, String), Vec<ColumnInfo>>,
    pub loading_tables: bool,

    // Sidebar & tabs
    pub sidebar: SidebarState,
    pub tabs: Vec<Tab>,
    pub active_tab_id: Option<u64>,
    next_id: u64,
    query_counter: usize,

    // Status bar toast
    pub toast: Option<Toast>,
}

impl DbViewerApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cmd_tx: UnboundedSender<DbCommand>,
        event_rx: StdReceiver<DbEvent>,
    ) -> Self {
        theme::apply_beekeeper_theme(&cc.egui_ctx);
        let saved = load_connections().unwrap_or_default();
        Self::new_with_connections(cmd_tx, event_rx, saved)
    }

    /// Headless-friendly constructor for integration tests and non-GUI state verification.
    pub fn new_with_connections(
        cmd_tx: UnboundedSender<DbCommand>,
        event_rx: StdReceiver<DbEvent>,
        saved_connections: Vec<ConnectionConfig>,
    ) -> Self {
        Self {
            cmd_tx,
            event_rx,
            connection_screen: ConnectionScreenState::new(saved_connections),
            active_connection: None,
            server_version: String::new(),
            databases: Vec::new(),
            current_database: None,
            tables: Vec::new(),
            columns_cache: HashMap::new(),
            loading_tables: false,
            sidebar: SidebarState::default(),
            tabs: Vec::new(),
            active_tab_id: None,
            next_id: 1,
            query_counter: 0,
            toast: None,
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send_cmd(&mut self, cmd: DbCommand) {
        if let Err(e) = self.cmd_tx.send(cmd) {
            self.set_toast(format!("Failed to send command to DB worker: {e}"), true);
        }
    }

    pub fn set_toast(&mut self, message: impl Into<String>, is_error: bool) {
        self.toast = Some(Toast {
            message: message.into(),
            is_error,
            created_at: Instant::now(),
        });
    }

    pub fn has_pending_async_work(&self) -> bool {
        self.connection_screen.is_busy()
            || self.loading_tables
            || !self.sidebar.loading_columns.is_empty()
            || self.tabs.iter().any(Tab::is_busy)
    }

    /// Drain all pending `DbEvent` messages without blocking.
    pub fn drain_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            self.process_event(event);
        }
    }

    /// Apply a single `DbEvent` to application state.
    pub fn process_event(&mut self, event: DbEvent) {
        match event {
            DbEvent::TestConnectionResult(res) => {
                self.connection_screen.testing = false;
                match res {
                    Ok(info) => {
                        self.connection_screen.status_banner = Some((false, info));
                    }
                    Err(err) => {
                        self.connection_screen.status_banner = Some((true, err));
                    }
                }
            }
            DbEvent::Connected {
                config,
                server_version,
                databases,
            } => {
                self.connection_screen.connecting = false;
                self.connection_screen.status_banner = None;
                let default_db = config.database.clone();
                self.active_connection = Some(config);
                self.server_version = server_version.clone();
                self.databases = databases;
                self.tables.clear();
                self.columns_cache.clear();
                self.sidebar.reset();
                self.tabs.clear();
                self.active_tab_id = None;
                self.query_counter = 0;

                // Open an initial SQL query tab so workspace feels immediately ready
                self.open_new_sql_tab();

                if let Some(db) = default_db.filter(|d| !d.is_empty()) {
                    self.current_database = Some(db.clone());
                    self.loading_tables = true;
                    self.send_cmd(DbCommand::UseDatabase { database: db });
                } else {
                    self.current_database = None;
                }

                self.set_toast(format!("Connected ({server_version})"), false);
            }
            DbEvent::Disconnected => {
                self.active_connection = None;
                self.server_version.clear();
                self.databases.clear();
                self.current_database = None;
                self.tables.clear();
                self.columns_cache.clear();
                self.loading_tables = false;
                self.sidebar.reset();
                self.tabs.clear();
                self.active_tab_id = None;
                self.set_toast("Disconnected", false);
            }
            DbEvent::DatabasesLoaded(databases) => {
                self.databases = databases;
            }
            DbEvent::DatabaseSwitched { database, tables } => {
                self.current_database = Some(database);
                self.tables = tables;
                self.loading_tables = false;
                self.sidebar.expanded_tables.clear();
                self.sidebar.loading_columns.clear();
            }
            DbEvent::TablesLoaded { database, tables } => {
                if self.current_database.as_deref() == Some(database.as_str()) {
                    self.tables = tables;
                    self.loading_tables = false;
                }
            }
            DbEvent::ColumnsLoaded {
                database,
                table,
                columns,
            } => {
                self.sidebar.loading_columns.remove(&table);
                self.columns_cache.insert((database, table), columns);
            }
            DbEvent::TableDataLoaded {
                request_id,
                database,
                table,
                page,
                page_size,
                result,
            } => {
                for tab in &mut self.tabs {
                    if let Tab::Table { state, .. } = tab {
                        if state.pending_request_id == Some(request_id)
                            || (state.database == database
                                && state.table == table
                                && state.loading)
                        {
                            state.set_result(page, page_size, result);
                            break;
                        }
                    }
                }
            }
            DbEvent::QueryResult { request_id, result } => {
                for tab in &mut self.tabs {
                    if let Tab::Sql(sql_tab) = tab {
                        if sql_tab.running_request_id == Some(request_id) {
                            sql_tab.set_result(result);
                            break;
                        }
                    }
                }
            }
            DbEvent::QueryCancelled { request_id } => {
                for tab in &mut self.tabs {
                    if let Tab::Sql(sql_tab) = tab {
                        if sql_tab.running_request_id == Some(request_id) {
                            sql_tab.running_request_id = None;
                            sql_tab.error = Some("Query cancelled.".to_string());
                            break;
                        }
                    }
                }
                self.set_toast("Query cancelled", false);
            }
            DbEvent::ChangesApplied {
                request_id,
                database,
                table,
                affected_rows,
            } => {
                let mut reload_target: Option<u64> = None;
                for tab in &mut self.tabs {
                    if let Tab::Table { id, state } = tab {
                        if state.pending_request_id == Some(request_id)
                            || (state.database == database && state.table == table)
                        {
                            state.discard_changes();
                            state.loading = false;
                            state.pending_request_id = None;
                            reload_target = Some(*id);
                            break;
                        }
                    }
                }
                self.set_toast(
                    format!("Applied changes to `{table}` ({affected_rows} rows affected)"),
                    false,
                );
                if let Some(tab_id) = reload_target {
                    self.reload_table_tab(tab_id);
                }
            }
            DbEvent::Error {
                request_id,
                message,
            } => {
                if self.connection_screen.connecting {
                    self.connection_screen.connecting = false;
                    self.connection_screen.status_banner = Some((true, message.clone()));
                }
                if self.connection_screen.testing {
                    self.connection_screen.testing = false;
                    self.connection_screen.status_banner = Some((true, message.clone()));
                }

                if let Some(req_id) = request_id {
                    for tab in &mut self.tabs {
                        match tab {
                            Tab::Table { state, .. }
                                if state.pending_request_id == Some(req_id) =>
                            {
                                state.loading = false;
                                state.pending_request_id = None;
                                state.error = Some(message.clone());
                            }
                            Tab::Sql(sql_tab) if sql_tab.running_request_id == Some(req_id) => {
                                sql_tab.running_request_id = None;
                                sql_tab.error = Some(message.clone());
                            }
                            _ => {}
                        }
                    }
                } else {
                    self.loading_tables = false;
                    self.sidebar.loading_columns.clear();
                }
                self.set_toast(message, true);
            }
        }
    }

    /// Open a table tab (focuses existing tab if already open for the same database + table).
    pub fn open_table_tab(&mut self, database: String, table: String) -> u64 {
        for tab in &self.tabs {
            if let Tab::Table { id, state } = tab {
                if state.database == database && state.table == table {
                    let id = *id;
                    self.active_tab_id = Some(id);
                    return id;
                }
            }
        }

        let tab_id = self.alloc_id();
        let grid = DataGridState::new(database, table);
        self.tabs.push(Tab::Table {
            id: tab_id,
            state: Box::new(grid),
        });
        self.active_tab_id = Some(tab_id);
        self.reload_table_tab(tab_id);
        tab_id
    }

    /// Create and focus a new SQL query tab.
    pub fn open_new_sql_tab(&mut self) -> u64 {
        self.query_counter += 1;
        let id = self.alloc_id();
        let tab = SqlTabState::new(id, self.query_counter);
        self.tabs.push(Tab::Sql(Box::new(tab)));
        self.active_tab_id = Some(id);
        id
    }

    /// Close a tab by ID and select a sensible neighbor.
    pub fn close_tab(&mut self, tab_id: u64) {
        let Some(pos) = self.tabs.iter().position(|t| t.id() == tab_id) else {
            return;
        };
        self.tabs.remove(pos);
        if self.active_tab_id == Some(tab_id) {
            self.active_tab_id = if self.tabs.is_empty() {
                None
            } else {
                let next_idx = pos.min(self.tabs.len() - 1);
                Some(self.tabs[next_idx].id())
            };
        }
    }

    /// Issue a `FetchTableData` command for the given table tab.
    pub fn reload_table_tab(&mut self, tab_id: u64) {
        let req_id = self.alloc_id();
        let cmd = self.tabs.iter_mut().find_map(|tab| match tab {
            Tab::Table { id, state } if *id == tab_id => {
                state.loading = true;
                state.pending_request_id = Some(req_id);
                state.error = None;
                Some(DbCommand::FetchTableData {
                    request_id: req_id,
                    database: state.database.clone(),
                    table: state.table.clone(),
                    page: state.page,
                    page_size: state.page_size,
                    sort: state.sort.clone(),
                    filters: state.active_filters.clone(),
                })
            }
            _ => None,
        });

        if let Some(cmd) = cmd {
            self.send_cmd(cmd);
        }
    }

    fn handle_connection_action(&mut self, action: ConnectionAction) {
        match action {
            ConnectionAction::Test(config) => {
                self.connection_screen.testing = true;
                self.send_cmd(DbCommand::TestConnection { config });
            }
            ConnectionAction::Connect(config) => {
                self.connection_screen.connecting = true;
                self.send_cmd(DbCommand::Connect { config });
            }
            ConnectionAction::Save(config) => {
                if let Some(existing) = self
                    .connection_screen
                    .saved_connections
                    .iter_mut()
                    .find(|c| c.id == config.id)
                {
                    *existing = config;
                } else {
                    self.connection_screen.saved_connections.push(config);
                }
                match save_connections(&self.connection_screen.saved_connections) {
                    Ok(()) => {
                        self.connection_screen.status_banner =
                            Some((false, "Connection saved.".to_string()));
                    }
                    Err(e) => {
                        self.connection_screen.status_banner =
                            Some((true, format!("Failed to save connections: {e}")));
                    }
                }
            }
            ConnectionAction::Delete(id) => {
                self.connection_screen
                    .saved_connections
                    .retain(|c| c.id != id);
                if self.connection_screen.selected_id.as_deref() == Some(id.as_str()) {
                    self.connection_screen.reset_new_form();
                }
                if let Err(e) = save_connections(&self.connection_screen.saved_connections) {
                    self.connection_screen.status_banner =
                        Some((true, format!("Failed to save connections: {e}")));
                }
            }
        }
    }

    fn handle_sidebar_action(&mut self, action: SidebarAction) {
        match action {
            SidebarAction::SwitchDatabase(db) => {
                self.current_database = Some(db.clone());
                self.loading_tables = true;
                self.send_cmd(DbCommand::UseDatabase { database: db });
            }
            SidebarAction::OpenTable(table) => {
                if let Some(db) = self.current_database.clone() {
                    self.open_table_tab(db, table);
                }
            }
            SidebarAction::LoadColumns(table) => {
                if let Some(db) = self.current_database.clone() {
                    self.send_cmd(DbCommand::ListColumns {
                        database: db,
                        table,
                    });
                }
            }
            SidebarAction::Refresh => {
                self.send_cmd(DbCommand::ListDatabases);
                if let Some(db) = self.current_database.clone() {
                    self.loading_tables = true;
                    self.columns_cache.clear();
                    self.send_cmd(DbCommand::ListTables { database: db });
                }
            }
            SidebarAction::Disconnect => {
                self.send_cmd(DbCommand::Disconnect);
            }
            SidebarAction::NewQuery => {
                self.open_new_sql_tab();
            }
        }
    }

    fn export_result(&mut self, result: &QueryResult, format: ExportFormat, path: &str) {
        let out_path = Path::new(path);
        let res = match format {
            ExportFormat::Csv => export_csv_file(result, out_path),
            ExportFormat::Json => export_json_file(result, out_path),
        };
        match res {
            Ok(()) => self.set_toast(format!("Exported {} to {path}", format.label()), false),
            Err(e) => self.set_toast(format!("Export failed: {e}"), true),
        }
    }

    fn show_connected_workspace(&mut self, ctx: &egui::Context) {
        // Bottom status bar
        egui::TopBottomPanel::bottom("status_bar")
            .exact_height(28.0)
            .frame(
                egui::Frame::none()
                    .fill(theme::BG_SIDEBAR)
                    .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE))
                    .inner_margin(egui::Margin::symmetric(10.0, 4.0)),
            )
            .show(ctx, |ui| {
                self.show_status_bar(ui);
            });

        // Left sidebar
        let conn_name = self
            .active_connection
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "Connected".to_string());

        let mut sidebar_action: Option<SidebarAction> = None;
        egui::SidePanel::left("connected_sidebar")
            .resizable(true)
            .default_width(250.0)
            .min_width(180.0)
            .max_width(420.0)
            .frame(
                egui::Frame::none()
                    .fill(theme::BG_SIDEBAR)
                    .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE))
                    .inner_margin(egui::Margin::same(10.0)),
            )
            .show(ctx, |ui| {
                sidebar_action = self.sidebar.show(
                    ui,
                    &conn_name,
                    &self.databases,
                    self.current_database.as_deref(),
                    &self.tables,
                    &self.columns_cache,
                    self.loading_tables,
                );
            });

        if let Some(action) = sidebar_action {
            self.handle_sidebar_action(action);
        }

        // Main panel: tab bar + active tab content
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme::BG_BASE))
            .show(ctx, |ui| {
                self.show_tab_bar(ui);
                ui.separator();

                egui::Frame::none()
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        self.show_active_tab(ui);
                    });
            });
    }

    fn show_tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut select_id: Option<u64> = None;
        let mut close_id: Option<u64> = None;
        let mut new_query = false;

        egui::Frame::none()
            .fill(theme::BG_SIDEBAR)
            .inner_margin(egui::Margin::symmetric(6.0, 4.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    for tab in &self.tabs {
                        let id = tab.id();
                        let is_active = self.active_tab_id == Some(id);
                        let fill = if is_active {
                            theme::BG_BASE
                        } else {
                            theme::BG_SURFACE
                        };
                        let stroke = if is_active {
                            Stroke::new(1.0_f32, theme::ACCENT)
                        } else {
                            Stroke::new(1.0_f32, theme::BORDER_SUBTLE)
                        };

                        egui::Frame::none()
                            .fill(fill)
                            .stroke(stroke)
                            .rounding(Rounding::same(4.0))
                            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 5.0;
                                    let icon = match tab {
                                        Tab::Table { .. } => "T",
                                        Tab::Sql(_) => ">_",
                                    };
                                    let icon_color = match tab {
                                        Tab::Table { .. } => theme::ACCENT,
                                        Tab::Sql(_) => theme::INFO,
                                    };
                                    ui.label(
                                        RichText::new(icon)
                                            .monospace()
                                            .strong()
                                            .color(icon_color)
                                            .size(11.0),
                                    );

                                    let title = if tab.is_dirty() {
                                        format!("{} *", tab.title())
                                    } else {
                                        tab.title().to_string()
                                    };
                                    let title_color = if is_active {
                                        theme::TEXT_PRIMARY
                                    } else {
                                        theme::TEXT_MUTED
                                    };
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(title)
                                                    .color(title_color)
                                                    .size(12.5),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        select_id = Some(id);
                                    }

                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("x")
                                                    .color(theme::TEXT_MUTED)
                                                    .size(12.0),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text("Close tab")
                                        .clicked()
                                    {
                                        close_id = Some(id);
                                    }
                                });
                            });
                    }

                    if ui
                        .button(RichText::new("+ New Query").color(theme::ACCENT).size(12.5))
                        .clicked()
                    {
                        new_query = true;
                    }
                });
            });

        if let Some(id) = select_id {
            self.active_tab_id = Some(id);
        }
        if let Some(id) = close_id {
            self.close_tab(id);
        }
        if new_query {
            self.open_new_sql_tab();
        }
    }

    fn show_active_tab(&mut self, ui: &mut egui::Ui) {
        let Some(active_id) = self.active_tab_id else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Select a table from the sidebar or open a new SQL query.")
                            .color(theme::TEXT_MUTED),
                    );
                    ui.add_space(8.0);
                    if theme::accent_button(ui, "+ New Query").clicked() {
                        self.open_new_sql_tab();
                    }
                });
            });
            return;
        };

        let Some(tab_idx) = self.tabs.iter().position(|t| t.id() == active_id) else {
            return;
        };

        enum TabOutcome {
            None,
            Table(u64, DataGridAction),
            Sql(usize, SqlEditorAction),
        }

        let outcome = match &mut self.tabs[tab_idx] {
            Tab::Table { id, state } => {
                let id = *id;
                match state.show(ui, id) {
                    Some(act) => TabOutcome::Table(id, act),
                    None => TabOutcome::None,
                }
            }
            Tab::Sql(sql_tab) => match sql_tab.show(ui) {
                Some(act) => TabOutcome::Sql(tab_idx, act),
                None => TabOutcome::None,
            },
        };

        match outcome {
            TabOutcome::None => {}
            TabOutcome::Table(tab_id, act) => match act {
                DataGridAction::Reload => self.reload_table_tab(tab_id),
                DataGridAction::ApplyChanges(changes) => {
                    let req_id = self.alloc_id();
                    if let Tab::Table { state, .. } = &mut self.tabs[tab_idx] {
                        state.loading = true;
                        state.pending_request_id = Some(req_id);
                        state.error = None;
                    }
                    self.send_cmd(DbCommand::ApplyTableChanges {
                        request_id: req_id,
                        changes,
                    });
                }
                DataGridAction::Export { format, path } => {
                    let result_clone = match &self.tabs[tab_idx] {
                        Tab::Table { state, .. } => state.result.clone(),
                        _ => None,
                    };
                    if let Some(res) = result_clone {
                        self.export_result(&res, format, &path);
                    }
                }
            },
            TabOutcome::Sql(idx, act) => match act {
                SqlEditorAction::Execute(sql) => {
                    let req_id = self.alloc_id();
                    if let Tab::Sql(sql_tab) = &mut self.tabs[idx] {
                        sql_tab.running_request_id = Some(req_id);
                        sql_tab.error = None;
                    }
                    self.send_cmd(DbCommand::ExecuteQuery {
                        request_id: req_id,
                        sql,
                    });
                }
                SqlEditorAction::Cancel(req_id) => {
                    self.send_cmd(DbCommand::CancelQuery { request_id: req_id });
                }
                SqlEditorAction::Export { format, path } => {
                    let result_clone = match &self.tabs[idx] {
                        Tab::Sql(sql_tab) => sql_tab.result.clone(),
                        _ => None,
                    };
                    if let Some(res) = result_clone {
                        self.export_result(&res, format, &path);
                    }
                }
            },
        }
    }

    fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        // Expire old toast automatically
        if let Some(toast) = &self.toast {
            if toast.created_at.elapsed() > Duration::from_secs(TOAST_DURATION_SECS) {
                self.toast = None;
            }
        }

        ui.horizontal(|ui| {
            // Left: connection & database info
            if let Some(conn) = &self.active_connection {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(rect.center(), 4.0, theme::SUCCESS);
                let db_part = self
                    .current_database
                    .as_deref()
                    .map(|d| format!(" / {d}"))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!(
                        "{}@{}:{}{}",
                        conn.username, conn.host, conn.port, db_part
                    ))
                    .monospace()
                    .size(11.5)
                    .color(theme::TEXT_PRIMARY),
                );
                if !self.server_version.is_empty() {
                    ui.separator();
                    ui.label(
                        RichText::new(&self.server_version)
                            .size(11.5)
                            .color(theme::TEXT_MUTED),
                    );
                }
            }

            if self.has_pending_async_work() {
                ui.separator();
                ui.spinner();
                ui.label(RichText::new("Working...").size(11.5).color(theme::ACCENT));
            }

            // Right: toast notification
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut dismiss = false;
                if let Some(toast) = &self.toast {
                    let color = if toast.is_error {
                        theme::DANGER
                    } else {
                        theme::SUCCESS
                    };
                    if ui
                        .add(
                            egui::Button::new(RichText::new("x").size(11.0).color(theme::TEXT_MUTED))
                                .frame(false),
                        )
                        .clicked()
                    {
                        dismiss = true;
                    }
                    ui.label(
                        RichText::new(&toast.message)
                            .size(11.5)
                            .color(color),
                    );
                }
                if dismiss {
                    self.toast = None;
                }
            });
        });

        let _ = Color32::TRANSPARENT;
    }
}

impl eframe::App for DbViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();

        if self.active_connection.is_some() {
            self.show_connected_workspace(ctx);
        } else if let Some(action) = self.connection_screen.show(ctx) {
            self.handle_connection_action(action);
        }

        // Request repaint while async work or toast expiration is pending
        if self.has_pending_async_work() {
            ctx.request_repaint_after(Duration::from_millis(32));
        } else if self.toast.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CellValue, TableKind};
    use std::sync::mpsc as std_mpsc;
    use tokio::sync::mpsc as tokio_mpsc;

    fn make_app() -> (
        DbViewerApp,
        tokio_mpsc::UnboundedReceiver<DbCommand>,
        std_mpsc::Sender<DbEvent>,
    ) {
        let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = std_mpsc::channel();
        let app = DbViewerApp::new_with_connections(cmd_tx, event_rx, Vec::new());
        (app, cmd_rx, event_tx)
    }

    fn sample_query_result() -> QueryResult {
        QueryResult {
            columns: vec![
                ColumnInfo {
                    name: "id".to_string(),
                    data_type: "INT".to_string(),
                    nullable: false,
                    is_primary_key: true,
                    default_value: None,
                },
                ColumnInfo {
                    name: "name".to_string(),
                    data_type: "VARCHAR(100)".to_string(),
                    nullable: false,
                    is_primary_key: false,
                    default_value: None,
                },
            ],
            rows: vec![
                vec![CellValue::Int(1), CellValue::Text("Alice".to_string())],
                vec![CellValue::Int(2), CellValue::Text("Bob".to_string())],
            ],
            total_rows: Some(250),
            affected_rows: None,
            execution_ms: 12,
            primary_key_columns: vec!["id".to_string()],
        }
    }

    #[test]
    fn connected_event_initializes_workspace_and_sql_tab() {
        let (mut app, mut cmd_rx, event_tx) = make_app();
        let cfg = ConnectionConfig {
            database: Some("shop".to_string()),
            ..Default::default()
        };

        event_tx
            .send(DbEvent::Connected {
                config: cfg,
                server_version: "8.0.36".to_string(),
                databases: vec![DatabaseInfo {
                    name: "shop".to_string(),
                }],
            })
            .unwrap();

        app.drain_events();

        assert!(app.active_connection.is_some());
        assert_eq!(app.current_database.as_deref(), Some("shop"));
        assert_eq!(app.tabs.len(), 1);
        assert!(matches!(app.tabs[0], Tab::Sql(_)));

        // Should have requested UseDatabase for default db
        let cmd = cmd_rx.try_recv().expect("expected UseDatabase command");
        assert!(matches!(cmd, DbCommand::UseDatabase { database } if database == "shop"));
    }

    #[test]
    fn table_tab_deduplicates_and_handles_edits_and_pagination() {
        let (mut app, mut cmd_rx, event_tx) = make_app();
        app.process_event(DbEvent::Connected {
            config: ConnectionConfig::default(),
            server_version: "11.4-MariaDB".to_string(),
            databases: vec![DatabaseInfo {
                name: "app".to_string(),
            }],
        });
        app.process_event(DbEvent::DatabaseSwitched {
            database: "app".to_string(),
            tables: vec![TableInfo {
                name: "users".to_string(),
                kind: TableKind::Table,
            }],
        });

        let tab_id = app.open_table_tab("app".to_string(), "users".to_string());
        // Opening same table again focuses existing tab instead of duplicating
        let tab_id_2 = app.open_table_tab("app".to_string(), "users".to_string());
        assert_eq!(tab_id, tab_id_2);

        let fetch_cmd = cmd_rx.try_recv().expect("expected FetchTableData");
        let req_id = match fetch_cmd {
            DbCommand::FetchTableData { request_id, .. } => request_id,
            other => panic!("unexpected command: {other:?}"),
        };

        event_tx
            .send(DbEvent::TableDataLoaded {
                request_id: req_id,
                database: "app".to_string(),
                table: "users".to_string(),
                page: 1,
                page_size: 100,
                result: sample_query_result(),
            })
            .unwrap();
        app.drain_events();

        let grid = app
            .tabs
            .iter_mut()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == tab_id => Some(state),
                _ => None,
            })
            .expect("table tab should exist");

        assert_eq!(grid.total_pages(), 3); // 250 / 100 = 3 pages
        assert!(grid.can_edit());

        // Stage edit, insert, delete
        grid.stage_edit(0, 1, CellValue::Text("Alicia".to_string()));
        grid.toggle_delete_row(1);
        grid.add_empty_row();
        assert_eq!(grid.pending_change_count(), 3);

        let changes = grid.build_table_changes().expect("should build changes");
        assert_eq!(changes.edits.len(), 1);
        assert_eq!(changes.deletes.len(), 1);
        assert_eq!(changes.inserts.len(), 1);
        assert_eq!(changes.edits[0].new_value, CellValue::Text("Alicia".to_string()));

        grid.discard_changes();
        assert!(!grid.has_pending_changes());
    }

    #[test]
    fn closing_active_tab_focuses_neighbor() {
        let (mut app, _cmd_rx, _event_tx) = make_app();
        let t1 = app.open_new_sql_tab();
        let t2 = app.open_new_sql_tab();
        assert_eq!(app.active_tab_id, Some(t2));
        app.close_tab(t2);
        assert_eq!(app.active_tab_id, Some(t1));
        app.close_tab(t1);
        assert_eq!(app.active_tab_id, None);
    }

    #[test]
    fn multi_and_filters_apply_and_clear() {
        use crate::models::FilterOperator;
        use crate::ui::data_grid::FilterRow;

        let mut grid = DataGridState::new("app".to_string(), "orders".to_string());
        grid.set_result(1, 100, sample_query_result());

        grid.filter_rows = vec![
            FilterRow {
                column: "id".to_string(),
                operator: FilterOperator::Eq,
                value: "205".to_string(),
            },
            FilterRow {
                column: "name".to_string(),
                operator: FilterOperator::Like,
                value: "%Ali%".to_string(),
            },
            FilterRow {
                column: "name".to_string(),
                operator: FilterOperator::IsNotNull,
                value: String::new(),
            },
        ];
        grid.apply_filter_rows();
        assert_eq!(grid.active_filters.len(), 3);
        assert_eq!(grid.active_filters[0].column, "id");
        assert_eq!(grid.active_filters[0].value, "205");
        assert_eq!(grid.active_filters[1].operator, FilterOperator::Like);
        assert_eq!(grid.active_filters[2].operator, FilterOperator::IsNotNull);

        grid.clear_filters();
        assert!(grid.active_filters.is_empty());
        assert_eq!(grid.filter_rows.len(), 1);
        assert_eq!(grid.filter_rows[0].column, "id");
    }

    #[test]
    fn sql_tab_client_side_sort_handles_numbers_text_and_nulls_last() {
        use crate::models::SortDirection;

        let mut tab = SqlTabState::new(1, 1);
        tab.set_result(QueryResult {
            columns: vec![
                ColumnInfo {
                    name: "score".to_string(),
                    data_type: "INT".to_string(),
                    nullable: true,
                    is_primary_key: false,
                    default_value: None,
                },
                ColumnInfo {
                    name: "name".to_string(),
                    data_type: "VARCHAR".to_string(),
                    nullable: true,
                    is_primary_key: false,
                    default_value: None,
                },
            ],
            rows: vec![
                vec![CellValue::Int(20), CellValue::Text("charlie".to_string())],
                vec![CellValue::Null, CellValue::Text("Alpha".to_string())],
                vec![CellValue::Int(5), CellValue::Null],
                vec![CellValue::Int(100), CellValue::Text("beta".to_string())],
            ],
            total_rows: None,
            affected_rows: None,
            execution_ms: 3,
            primary_key_columns: Vec::new(),
        });

        // Sort score ASC -> 5 (idx 2), 20 (idx 0), 100 (idx 3), NULL (idx 1)
        tab.toggle_sort("score".to_string());
        assert_eq!(
            tab.sort.as_ref().map(|s| s.direction),
            Some(SortDirection::Asc)
        );
        assert_eq!(tab.sorted_row_indices, vec![2, 0, 3, 1]);

        // Sort score DESC -> 100 (idx 3), 20 (idx 0), 5 (idx 2), NULL (idx 1)
        tab.toggle_sort("score".to_string());
        assert_eq!(
            tab.sort.as_ref().map(|s| s.direction),
            Some(SortDirection::Desc)
        );
        assert_eq!(tab.sorted_row_indices, vec![3, 0, 2, 1]);

        // 3rd click clears sort -> original order
        tab.toggle_sort("score".to_string());
        assert!(tab.sort.is_none());
        assert_eq!(tab.sorted_row_indices, vec![0, 1, 2, 3]);

        // Sort name ASC (case-insensitive, NULL last) -> Alpha (1), beta (3), charlie (0), NULL (2)
        tab.toggle_sort("name".to_string());
        assert_eq!(tab.sorted_row_indices, vec![1, 3, 0, 2]);
    }

    #[test]
    fn sidebar_filter_ranks_exact_then_prefix_then_substring() {
        use crate::ui::sidebar::filter_and_sort_tables;

        let tables = vec![
            TableInfo {
                name: "my_users_log".to_string(),
                kind: TableKind::Table,
            },
            TableInfo {
                name: "users_archive".to_string(),
                kind: TableKind::Table,
            },
            TableInfo {
                name: "users".to_string(),
                kind: TableKind::Table,
            },
            TableInfo {
                name: "orders".to_string(),
                kind: TableKind::Table,
            },
        ];

        let matched: Vec<&str> = filter_and_sort_tables(&tables, "users")
            .into_iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(matched, vec!["users", "users_archive", "my_users_log"]);
    }
}
