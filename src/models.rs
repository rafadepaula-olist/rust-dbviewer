use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

fn default_connect_timeout() -> u64 {
    10
}

/// Saved or active MySQL/MariaDB connection configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: Option<String>,
    #[serde(default)]
    pub use_ssl: bool,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    #[serde(default)]
    pub color: Option<String>,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: "Local MySQL".to_string(),
            host: "127.0.0.1".to_string(),
            port: 3306,
            username: "root".to_string(),
            password: String::new(),
            database: None,
            use_ssl: false,
            connect_timeout_secs: default_connect_timeout(),
            color: None,
        }
    }
}

impl ConnectionConfig {
    pub fn new(
        name: impl Into<String>,
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        password: impl Into<String>,
        database: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            host: host.into(),
            port,
            username: username.into(),
            password: password.into(),
            database,
            use_ssl: false,
            connect_timeout_secs: default_connect_timeout(),
            color: None,
        }
    }
}

/// Metadata for a database/schema on the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseInfo {
    pub name: String,
}

/// Whether a relation is a base table or a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableKind {
    Table,
    View,
}

/// Metadata for a table or view inside a database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub kind: TableKind,
}

/// Column schema metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub default_value: Option<String>,
}

/// Sort direction for ORDER BY clauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub fn as_sql(&self) -> &'static str {
        match self {
            SortDirection::Asc => "ASC",
            SortDirection::Desc => "DESC",
        }
    }

    pub fn toggle(&self) -> Self {
        match self {
            SortDirection::Asc => SortDirection::Desc,
            SortDirection::Desc => SortDirection::Asc,
        }
    }
}

/// Sort specification for table browsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortSpec {
    pub column: String,
    pub direction: SortDirection,
}

/// Supported filter operators for table browsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterOperator {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    NotLike,
    IsNull,
    IsNotNull,
}

impl FilterOperator {
    pub const ALL: &'static [FilterOperator] = &[
        FilterOperator::Eq,
        FilterOperator::Neq,
        FilterOperator::Gt,
        FilterOperator::Gte,
        FilterOperator::Lt,
        FilterOperator::Lte,
        FilterOperator::Like,
        FilterOperator::NotLike,
        FilterOperator::IsNull,
        FilterOperator::IsNotNull,
    ];

    pub fn as_sql(&self) -> &'static str {
        match self {
            FilterOperator::Eq => "=",
            FilterOperator::Neq => "!=",
            FilterOperator::Gt => ">",
            FilterOperator::Gte => ">=",
            FilterOperator::Lt => "<",
            FilterOperator::Lte => "<=",
            FilterOperator::Like => "LIKE",
            FilterOperator::NotLike => "NOT LIKE",
            FilterOperator::IsNull => "IS NULL",
            FilterOperator::IsNotNull => "IS NOT NULL",
        }
    }

    pub fn takes_value(&self) -> bool {
        !matches!(self, FilterOperator::IsNull | FilterOperator::IsNotNull)
    }
}

impl fmt::Display for FilterOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_sql())
    }
}

/// Filter specification for table browsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterSpec {
    pub column: String,
    pub operator: FilterOperator,
    pub value: String,
}

/// Typed representation of a single database cell value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CellValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

impl CellValue {
    pub fn is_null(&self) -> bool {
        matches!(self, CellValue::Null)
    }

    /// Human-readable display string for grid cells.
    pub fn to_display_string(&self) -> String {
        match self {
            CellValue::Null => "NULL".to_string(),
            CellValue::Bool(b) => b.to_string(),
            CellValue::Int(i) => i.to_string(),
            CellValue::Float(v) => v.to_string(),
            CellValue::Text(s) => s.clone(),
            CellValue::Bytes(bytes) => {
                if bytes.is_empty() {
                    "0x".to_string()
                } else {
                    let mut hex = String::with_capacity(2 + bytes.len() * 2);
                    hex.push_str("0x");
                    for b in bytes {
                        hex.push_str(&format!("{b:02x}"));
                    }
                    hex
                }
            }
        }
    }

    /// String suitable for initializing an inline cell editor.
    pub fn to_edit_string(&self) -> String {
        match self {
            CellValue::Null => String::new(),
            _ => self.to_display_string(),
        }
    }

    /// Convert cell value into a `serde_json::Value` for JSON export.
    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            CellValue::Null => serde_json::Value::Null,
            CellValue::Bool(b) => serde_json::Value::Bool(*b),
            CellValue::Int(i) => serde_json::Value::Number((*i).into()),
            CellValue::Float(v) => serde_json::Number::from_f64(*v)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            CellValue::Text(s) => serde_json::Value::String(s.clone()),
            CellValue::Bytes(_) => serde_json::Value::String(self.to_display_string()),
        }
    }
}

impl fmt::Display for CellValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_display_string())
    }
}

/// Result of a SELECT, SHOW, DESCRIBE, or DML/DDL statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<CellValue>>,
    pub total_rows: Option<u64>,
    pub affected_rows: Option<u64>,
    pub execution_ms: u64,
    pub primary_key_columns: Vec<String>,
}

impl QueryResult {
    pub fn empty() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            total_rows: None,
            affected_rows: None,
            execution_ms: 0,
            primary_key_columns: Vec::new(),
        }
    }
}

/// A single modified cell pending commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingEdit {
    pub row_index: usize,
    pub col_index: usize,
    pub new_value: CellValue,
}

/// A new row pending insertion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingInsert {
    pub values: Vec<(String, CellValue)>,
}

/// An existing row marked for deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDelete {
    pub row_index: usize,
}

/// Batched inline edits, inserts, and deletes for a single table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableChanges {
    pub database: String,
    pub table: String,
    pub columns: Vec<ColumnInfo>,
    pub primary_key_columns: Vec<String>,
    pub original_rows: Vec<Vec<CellValue>>,
    pub edits: Vec<PendingEdit>,
    pub inserts: Vec<PendingInsert>,
    pub deletes: Vec<PendingDelete>,
}

impl TableChanges {
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty() && self.inserts.is_empty() && self.deletes.is_empty()
    }
}

/// Commands sent from the UI thread to the async DB worker.
#[derive(Debug, Clone)]
pub enum DbCommand {
    TestConnection {
        config: ConnectionConfig,
    },
    Connect {
        config: ConnectionConfig,
    },
    Disconnect,
    ListDatabases,
    UseDatabase {
        database: String,
    },
    ListTables {
        database: String,
    },
    ListColumns {
        database: String,
        table: String,
    },
    FetchTableData {
        request_id: u64,
        database: String,
        table: String,
        page: u64,
        page_size: u64,
        sort: Option<SortSpec>,
        filters: Vec<FilterSpec>,
    },
    ExecuteQuery {
        request_id: u64,
        sql: String,
    },
    CancelQuery {
        request_id: u64,
    },
    ApplyTableChanges {
        request_id: u64,
        changes: TableChanges,
    },
    Shutdown,
}

/// Events sent from the async DB worker back to the UI thread.
#[derive(Debug, Clone)]
pub enum DbEvent {
    TestConnectionResult(Result<String, String>),
    Connected {
        config: ConnectionConfig,
        server_version: String,
        databases: Vec<DatabaseInfo>,
    },
    Disconnected,
    DatabasesLoaded(Vec<DatabaseInfo>),
    DatabaseSwitched {
        database: String,
        tables: Vec<TableInfo>,
    },
    TablesLoaded {
        database: String,
        tables: Vec<TableInfo>,
    },
    ColumnsLoaded {
        database: String,
        table: String,
        columns: Vec<ColumnInfo>,
    },
    TableDataLoaded {
        request_id: u64,
        database: String,
        table: String,
        page: u64,
        page_size: u64,
        result: QueryResult,
    },
    QueryResult {
        request_id: u64,
        result: QueryResult,
    },
    QueryCancelled {
        request_id: u64,
    },
    ChangesApplied {
        request_id: u64,
        database: String,
        table: String,
        affected_rows: u64,
    },
    Error {
        request_id: Option<u64>,
        message: String,
    },
}
