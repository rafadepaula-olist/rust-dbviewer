use crate::db::query::{
    build_delete_queries, build_insert_queries, build_table_data_queries, build_update_queries,
    is_result_set_query, BuiltQuery,
};
use crate::models::{
    CellValue, ColumnInfo, ConnectionConfig, DatabaseInfo, FilterSpec, QueryResult, SortSpec,
    TableChanges, TableInfo, TableKind,
};
use sqlx::mysql::{MySqlArguments, MySqlConnectOptions, MySqlPoolOptions, MySqlRow, MySqlSslMode};
use sqlx::types::{chrono, BigDecimal};
use sqlx::{Column, Executor, MySql, MySqlPool, Row, TypeInfo, ValueRef};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Active MySQL/MariaDB client backed by a `sqlx::MySqlPool`.
pub struct MysqlClient {
    pool: MySqlPool,
    config: ConnectionConfig,
    current_database: Option<String>,
}

impl MysqlClient {
    /// Build `sqlx::mysql::MySqlConnectOptions` from a `ConnectionConfig`.
    pub fn build_connect_options(
        config: &ConnectionConfig,
        database_override: Option<&str>,
    ) -> MySqlConnectOptions {
        let ssl_mode = if config.use_ssl {
            MySqlSslMode::VerifyIdentity
        } else {
            MySqlSslMode::Disabled
        };

        let mut opts = MySqlConnectOptions::new()
            .host(&config.host)
            .port(config.port)
            .username(&config.username)
            .password(&config.password)
            .ssl_mode(ssl_mode);

        let db = database_override.or(config.database.as_deref());
        if let Some(db_name) = db.filter(|s| !s.trim().is_empty()) {
            opts = opts.database(db_name);
        }

        opts
    }

    /// Test connectivity to a MySQL/MariaDB server and return its version string.
    pub async fn test_connection(config: &ConnectionConfig) -> Result<String, String> {
        let opts = Self::build_connect_options(config, None);
        let timeout = Duration::from_secs(config.connect_timeout_secs.max(1));

        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(timeout)
            .connect_with(opts)
            .await
            .map_err(format_sqlx_error)?;

        let version = fetch_server_version(&pool).await;
        pool.close().await;
        version
    }

    /// Connect to a MySQL/MariaDB server, returning the client, server version, and database list.
    pub async fn connect(
        config: ConnectionConfig,
    ) -> Result<(Self, String, Vec<DatabaseInfo>), String> {
        let opts = Self::build_connect_options(&config, None);
        let timeout = Duration::from_secs(config.connect_timeout_secs.max(1));

        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(timeout)
            .connect_with(opts)
            .await
            .map_err(format_sqlx_error)?;

        let version = fetch_server_version(&pool).await?;
        let current_database = config
            .database
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .cloned();

        let client = Self {
            pool,
            config,
            current_database,
        };
        let databases = client.list_databases().await?;
        Ok((client, version, databases))
    }

    /// Access a clone of the underlying connection pool (cheap `Arc` clone).
    pub fn pool(&self) -> MySqlPool {
        self.pool.clone()
    }

    /// Close the connection pool cleanly.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// List all databases visible to the connected user (`SHOW DATABASES`).
    pub async fn list_databases(&self) -> Result<Vec<DatabaseInfo>, String> {
        let rows = sqlx::raw_sql("SHOW DATABASES")
            .fetch_all(&self.pool)
            .await
            .map_err(format_sqlx_error)?;

        let mut databases = Vec::with_capacity(rows.len());
        for row in rows {
            let name = decode_cell(&row, 0).to_display_string();
            databases.push(DatabaseInfo { name });
        }
        Ok(databases)
    }

    /// Switch the default database by rebuilding the pool bound to `database`, then list its tables.
    pub async fn use_database(&mut self, database: &str) -> Result<Vec<TableInfo>, String> {
        let opts = Self::build_connect_options(&self.config, Some(database));
        let timeout = Duration::from_secs(self.config.connect_timeout_secs.max(1));

        let new_pool = MySqlPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(timeout)
            .connect_with(opts)
            .await
            .map_err(format_sqlx_error)?;

        let old_pool = std::mem::replace(&mut self.pool, new_pool);
        self.current_database = Some(database.to_string());
        old_pool.close().await;

        self.list_tables(database).await
    }

    /// List base tables and views in `database`.
    pub async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>, String> {
        list_tables_on_pool(&self.pool, database).await
    }

    /// List columns (with primary key flags) for `database`.`table`.
    pub async fn list_columns(
        &self,
        database: &str,
        table: &str,
    ) -> Result<Vec<ColumnInfo>, String> {
        list_columns_on_pool(&self.pool, database, table).await
    }

    /// Apply batched inserts, updates, and deletes inside a single database transaction.
    pub async fn apply_table_changes(&self, changes: &TableChanges) -> Result<u64, String> {
        apply_table_changes_on_pool(&self.pool, changes).await
    }
}

async fn fetch_server_version(pool: &MySqlPool) -> Result<String, String> {
    let row = sqlx::raw_sql("SELECT VERSION() AS v")
        .fetch_one(pool)
        .await
        .map_err(format_sqlx_error)?;
    Ok(decode_cell(&row, 0).to_display_string())
}

pub async fn list_tables_on_pool(
    pool: &MySqlPool,
    database: &str,
) -> Result<Vec<TableInfo>, String> {
    let sql = "SELECT CAST(TABLE_NAME AS CHAR) AS name, CAST(TABLE_TYPE AS CHAR) AS table_type \
               FROM information_schema.TABLES \
               WHERE TABLE_SCHEMA = ? \
               ORDER BY TABLE_NAME";

    let rows = sqlx::query(sql)
        .bind(database)
        .fetch_all(pool)
        .await
        .map_err(format_sqlx_error)?;

    let mut tables = Vec::with_capacity(rows.len());
    for row in rows {
        let name = decode_cell(&row, 0).to_display_string();
        let table_type = decode_cell(&row, 1).to_display_string();
        let kind = if table_type.to_ascii_uppercase().contains("VIEW") {
            TableKind::View
        } else {
            TableKind::Table
        };
        tables.push(TableInfo { name, kind });
    }
    Ok(tables)
}

pub async fn list_primary_keys_on_pool(
    pool: &MySqlPool,
    database: &str,
    table: &str,
) -> Result<Vec<String>, String> {
    let sql = "SELECT CAST(COLUMN_NAME AS CHAR) AS column_name \
               FROM information_schema.KEY_COLUMN_USAGE \
               WHERE TABLE_SCHEMA = ? \
                 AND TABLE_NAME = ? \
                 AND CONSTRAINT_NAME = 'PRIMARY' \
               ORDER BY ORDINAL_POSITION";

    let rows = sqlx::query(sql)
        .bind(database)
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(format_sqlx_error)?;

    Ok(rows
        .iter()
        .map(|r| decode_cell(r, 0).to_display_string())
        .collect())
}

pub async fn list_columns_on_pool(
    pool: &MySqlPool,
    database: &str,
    table: &str,
) -> Result<Vec<ColumnInfo>, String> {
    let (columns, _) = list_columns_with_pks_on_pool(pool, database, table).await?;
    Ok(columns)
}

async fn list_columns_with_pks_on_pool(
    pool: &MySqlPool,
    database: &str,
    table: &str,
) -> Result<(Vec<ColumnInfo>, Vec<String>), String> {
    let pk_list = list_primary_keys_on_pool(pool, database, table).await?;
    let pk_set: HashSet<&str> = pk_list.iter().map(String::as_str).collect();

    let sql = "SELECT \
                 CAST(COLUMN_NAME AS CHAR) AS column_name, \
                 CAST(COLUMN_TYPE AS CHAR) AS column_type, \
                 CAST(IS_NULLABLE AS CHAR) AS is_nullable, \
                 CAST(COLUMN_KEY AS CHAR) AS column_key, \
                 CAST(COLUMN_DEFAULT AS CHAR) AS column_default \
               FROM information_schema.COLUMNS \
               WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
               ORDER BY ORDINAL_POSITION";

    let rows = sqlx::query(sql)
        .bind(database)
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(format_sqlx_error)?;

    let mut columns = Vec::with_capacity(rows.len());
    for row in rows {
        let name = decode_cell(&row, 0).to_display_string();
        let data_type = decode_cell(&row, 1).to_display_string();
        let nullable = decode_cell(&row, 2)
            .to_display_string()
            .eq_ignore_ascii_case("YES");
        let column_key = decode_cell(&row, 3).to_display_string();
        let is_primary_key = pk_set.contains(name.as_str()) || column_key.eq_ignore_ascii_case("PRI");
        let default_val = decode_cell(&row, 4);
        let default_value = if default_val.is_null() {
            None
        } else {
            Some(default_val.to_display_string())
        };

        columns.push(ColumnInfo {
            name,
            data_type,
            nullable,
            is_primary_key,
            default_value,
        });
    }

    Ok((columns, pk_list))
}

/// Fetch a paginated slice of rows plus total row count for `database`.`table`.
pub async fn fetch_table_data_on_pool(
    pool: &MySqlPool,
    database: &str,
    table: &str,
    page: u64,
    page_size: u64,
    sort: Option<&SortSpec>,
    filters: &[FilterSpec],
) -> Result<QueryResult, String> {
    let start = Instant::now();

    let (columns, primary_key_columns) =
        list_columns_with_pks_on_pool(pool, database, table).await?;

    let (select_q, count_q) =
        build_table_data_queries(database, table, page, page_size, sort, filters);

    let count_row = build_sqlx_query(&count_q)
        .fetch_one(pool)
        .await
        .map_err(format_sqlx_error)?;
    let total_rows = match decode_cell(&count_row, 0) {
        CellValue::Int(n) => u64::try_from(n).unwrap_or(0),
        other => other.to_display_string().parse::<u64>().unwrap_or(0),
    };

    let data_rows = build_sqlx_query(&select_q)
        .fetch_all(pool)
        .await
        .map_err(format_sqlx_error)?;

    let mut rows = Vec::with_capacity(data_rows.len());
    for row in &data_rows {
        let col_count = row.len();
        let mut cells = Vec::with_capacity(col_count);
        for idx in 0..col_count {
            cells.push(decode_cell(row, idx));
        }
        rows.push(cells);
    }

    let final_columns = if columns.is_empty() {
        data_rows
            .first()
            .map(columns_from_row)
            .unwrap_or_default()
    } else {
        columns
    };

    Ok(QueryResult {
        columns: final_columns,
        rows,
        total_rows: Some(total_rows),
        affected_rows: None,
        execution_ms: start.elapsed().as_millis() as u64,
        primary_key_columns,
    })
}

/// Execute an arbitrary SQL query (SELECT, SHOW, DESCRIBE, DML, DDL).
///
/// Records the MySQL `CONNECTION_ID()` into `connection_id_out` so callers can issue
/// `KILL QUERY <id>` if the query is cancelled while running.
pub async fn execute_raw_query_on_pool(
    pool: &MySqlPool,
    sql: &str,
    connection_id_out: Arc<AtomicU64>,
) -> Result<QueryResult, String> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err("Query is empty".to_string());
    }

    let start = Instant::now();
    let mut conn = pool.acquire().await.map_err(format_sqlx_error)?;

    if let Ok(id_row) = conn.fetch_one("SELECT CONNECTION_ID()").await {
        if let CellValue::Int(id) = decode_cell(&id_row, 0) {
            if let Ok(uid) = u64::try_from(id) {
                connection_id_out.store(uid, Ordering::SeqCst);
            }
        }
    }

    let is_select_like = is_result_set_query(trimmed);
    let mut columns: Vec<ColumnInfo> = Vec::new();
    let mut rows: Vec<Vec<CellValue>> = Vec::new();
    let mut affected_rows: Option<u64> = None;

    if is_select_like {
        let fetched = conn.fetch_all(trimmed).await.map_err(format_sqlx_error)?;

        if let Some(first) = fetched.first() {
            columns = columns_from_row(first);
        }
        rows.reserve(fetched.len());
        for row in &fetched {
            let mut cells = Vec::with_capacity(row.len());
            for idx in 0..row.len() {
                cells.push(decode_cell(row, idx));
            }
            rows.push(cells);
        }

        // For a SELECT that returned 0 rows, try `describe` to populate column headers.
        if columns.is_empty() {
            if let Ok(desc) = pool.describe(trimmed).await {
                columns = desc
                    .columns()
                    .iter()
                    .map(|c| ColumnInfo {
                        name: c.name().to_string(),
                        data_type: c.type_info().name().to_string(),
                        nullable: true,
                        is_primary_key: false,
                        default_value: None,
                    })
                    .collect();
            }
        }
    } else {
        let done = conn.execute(trimmed).await.map_err(format_sqlx_error)?;
        affected_rows = Some(done.rows_affected());
    }

    let total_rows = if is_select_like {
        Some(rows.len() as u64)
    } else {
        None
    };

    Ok(QueryResult {
        columns,
        rows,
        total_rows,
        affected_rows,
        execution_ms: start.elapsed().as_millis() as u64,
        primary_key_columns: Vec::new(),
    })
}

/// Apply batched inserts, updates, and deletes inside a single transaction.
pub async fn apply_table_changes_on_pool(
    pool: &MySqlPool,
    changes: &TableChanges,
) -> Result<u64, String> {
    if changes.is_empty() {
        return Ok(0);
    }

    let insert_queries = build_insert_queries(changes)?;
    let update_queries = build_update_queries(changes)?;
    let delete_queries = build_delete_queries(changes)?;

    let mut tx = pool.begin().await.map_err(format_sqlx_error)?;
    let mut total_affected: u64 = 0;

    for q in insert_queries
        .iter()
        .chain(update_queries.iter())
        .chain(delete_queries.iter())
    {
        let res = build_sqlx_query(q)
            .execute(&mut *tx)
            .await
            .map_err(format_sqlx_error)?;
        total_affected += res.rows_affected();
    }

    tx.commit().await.map_err(format_sqlx_error)?;
    Ok(total_affected)
}

fn build_sqlx_query<'q>(
    built: &'q BuiltQuery,
) -> sqlx::query::Query<'q, MySql, MySqlArguments> {
    let mut query = sqlx::query(&built.sql);
    for param in &built.params {
        query = match param {
            CellValue::Null => query.bind(Option::<String>::None),
            CellValue::Bool(b) => query.bind(*b),
            CellValue::Int(i) => query.bind(*i),
            CellValue::Float(f) => query.bind(*f),
            CellValue::Text(s) => query.bind(s.as_str()),
            CellValue::Bytes(bytes) => query.bind(bytes.as_slice()),
        };
    }
    query
}

fn columns_from_row(row: &MySqlRow) -> Vec<ColumnInfo> {
    row.columns()
        .iter()
        .map(|col| ColumnInfo {
            name: col.name().to_string(),
            data_type: col.type_info().name().to_string(),
            nullable: true,
            is_primary_key: false,
            default_value: None,
        })
        .collect()
}

/// Decode any MySQL/MariaDB column value into a `CellValue` without panicking.
pub fn decode_cell(row: &MySqlRow, idx: usize) -> CellValue {
    let raw = match row.try_get_raw(idx) {
        Ok(r) => r,
        Err(_) => return CellValue::Null,
    };
    if raw.is_null() {
        return CellValue::Null;
    }

    let type_name = row
        .columns()
        .get(idx)
        .map(|c| c.type_info().name().to_ascii_uppercase())
        .unwrap_or_default();

    match type_name.as_str() {
        "BOOLEAN" | "BOOL" => {
            if let Ok(v) = row.try_get::<bool, _>(idx) {
                return CellValue::Bool(v);
            }
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return CellValue::Int(v);
            }
        }
        "TINYINT" | "SMALLINT" | "INT" | "MEDIUMINT" | "BIGINT" => {
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return CellValue::Int(v);
            }
            if let Ok(v) = row.try_get::<u64, _>(idx) {
                return u64_to_cell(v);
            }
        }
        "TINYINT UNSIGNED"
        | "SMALLINT UNSIGNED"
        | "INT UNSIGNED"
        | "MEDIUMINT UNSIGNED"
        | "BIGINT UNSIGNED" => {
            if let Ok(v) = row.try_get::<u64, _>(idx) {
                return u64_to_cell(v);
            }
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return CellValue::Int(v);
            }
        }
        "FLOAT" => {
            if let Ok(v) = row.try_get::<f32, _>(idx) {
                return CellValue::Float(f64::from(v));
            }
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return CellValue::Float(v);
            }
        }
        "DOUBLE" => {
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return CellValue::Float(v);
            }
        }
        "DECIMAL" | "NUMERIC" | "NEWDECIMAL" => {
            if let Ok(v) = row.try_get::<BigDecimal, _>(idx) {
                return CellValue::Text(v.to_string());
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return CellValue::Text(v);
            }
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return CellValue::Float(v);
            }
        }
        "DATE" => {
            if let Ok(v) = row.try_get::<chrono::NaiveDate, _>(idx) {
                return CellValue::Text(v.to_string());
            }
        }
        "DATETIME" => {
            if let Ok(v) = row.try_get::<chrono::NaiveDateTime, _>(idx) {
                return CellValue::Text(v.format("%Y-%m-%d %H:%M:%S%.f").to_string());
            }
        }
        "TIMESTAMP" => {
            if let Ok(v) = row.try_get::<chrono::DateTime<chrono::Utc>, _>(idx) {
                return CellValue::Text(v.format("%Y-%m-%d %H:%M:%S%.f").to_string());
            }
            if let Ok(v) = row.try_get::<chrono::NaiveDateTime, _>(idx) {
                return CellValue::Text(v.format("%Y-%m-%d %H:%M:%S%.f").to_string());
            }
        }
        "TIME" => {
            if let Ok(v) = row.try_get::<chrono::NaiveTime, _>(idx) {
                return CellValue::Text(v.to_string());
            }
        }
        "YEAR" => {
            if let Ok(v) = row.try_get::<u16, _>(idx) {
                return CellValue::Int(i64::from(v));
            }
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return CellValue::Int(v);
            }
        }
        "JSON" => {
            if let Ok(v) = row.try_get::<serde_json::Value, _>(idx) {
                return CellValue::Text(v.to_string());
            }
        }
        "BIT" => {
            if let Ok(v) = row.try_get::<u64, _>(idx) {
                return u64_to_cell(v);
            }
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return CellValue::Bytes(v);
            }
        }
        "BINARY" | "VARBINARY" | "TINYBLOB" | "BLOB" | "MEDIUMBLOB" | "LONGBLOB" | "GEOMETRY" => {
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return CellValue::Bytes(v);
            }
        }
        _ => {}
    }

    // Generic fallback for strings, unknown types, and text-protocol representations.
    if let Ok(s) = row.try_get::<String, _>(idx) {
        return CellValue::Text(s);
    }
    if let Ok(bytes) = row.try_get::<Vec<u8>, _>(idx) {
        return match String::from_utf8(bytes) {
            Ok(s) => CellValue::Text(s),
            Err(err) => CellValue::Bytes(err.into_bytes()),
        };
    }

    CellValue::Text("<unreadable>".to_string())
}

fn u64_to_cell(v: u64) -> CellValue {
    match i64::try_from(v) {
        Ok(i) => CellValue::Int(i),
        Err(_) => CellValue::Text(v.to_string()),
    }
}

/// Produce a clean, user-friendly error message from a `sqlx::Error`.
pub fn format_sqlx_error(err: sqlx::Error) -> String {
    match err {
        sqlx::Error::Database(db_err) => {
            if let Some(code) = db_err.code() {
                format!("MySQL error ({code}): {}", db_err.message())
            } else {
                format!("MySQL error: {}", db_err.message())
            }
        }
        sqlx::Error::PoolTimedOut => {
            "Connection timed out while waiting for the MySQL server".to_string()
        }
        sqlx::Error::Io(io_err) => format!("Network error: {io_err}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u64_to_cell() {
        assert_eq!(u64_to_cell(42), CellValue::Int(42));
        assert_eq!(u64_to_cell(i64::MAX as u64), CellValue::Int(i64::MAX));
        assert_eq!(
            u64_to_cell(u64::MAX),
            CellValue::Text(u64::MAX.to_string())
        );
    }

    #[test]
    fn test_format_sqlx_error_timeout() {
        let msg = format_sqlx_error(sqlx::Error::PoolTimedOut);
        assert!(msg.contains("timed out"));
    }

    #[test]
    fn test_build_connect_options_does_not_panic() {
        let mut cfg = ConnectionConfig {
            database: Some("testdb".to_string()),
            ..Default::default()
        };
        let _opts = MysqlClient::build_connect_options(&cfg, None);
        cfg.use_ssl = true;
        let _opts_ssl = MysqlClient::build_connect_options(&cfg, Some("otherdb"));
    }
}
