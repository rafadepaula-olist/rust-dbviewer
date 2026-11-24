use rust_dbviewer::db::mysql::{
    apply_table_changes_on_pool, execute_raw_query_on_pool, fetch_table_data_on_pool, MysqlClient,
};
use rust_dbviewer::db::spawn_db_worker;
use rust_dbviewer::models::{
    CellValue, ConnectionConfig, DbCommand, DbEvent, FilterOperator, FilterSpec, PendingDelete,
    PendingEdit, PendingInsert, SortDirection, SortSpec, TableChanges, TableKind,
};
use std::process::Command;
use std::sync::atomic::AtomicU64;
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;

struct EphemeralMariaDb {
    container_id: String,
    port: u16,
}

impl EphemeralMariaDb {
    async fn start() -> Option<Self> {
        let output = Command::new("docker")
            .args([
                "run",
                "-d",
                "-P",
                "-e",
                "MARIADB_ROOT_PASSWORD=testpass",
                "-e",
                "MARIADB_DATABASE=testdb",
                "mariadb:11.2.4",
            ])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if container_id.is_empty() {
            return None;
        }

        let port_out = Command::new("docker")
            .args(["port", &container_id, "3306/tcp"])
            .output()
            .ok()?;
        let port_str = String::from_utf8_lossy(&port_out.stdout);
        let port: u16 = port_str
            .lines()
            .next()?
            .rsplit(':')
            .next()?
            .trim()
            .parse()
            .ok()?;

        let instance = Self { container_id, port };

        // Wait up to 30 seconds for MariaDB to accept connections.
        let cfg = instance.config(Some("testdb"));
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if MysqlClient::test_connection(&cfg).await.is_ok() {
                return Some(instance);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        None
    }

    fn config(&self, database: Option<&str>) -> ConnectionConfig {
        let mut cfg = ConnectionConfig::new(
            "Test MariaDB",
            "127.0.0.1",
            self.port,
            "root",
            "testpass",
            database.map(ToString::to_string),
        );
        cfg.connect_timeout_secs = 5;
        cfg
    }
}

impl Drop for EphemeralMariaDb {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.container_id])
            .output();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integration_mysql_full_lifecycle() {
    let Some(db) = EphemeralMariaDb::start().await else {
        eprintln!("Skipping integration_mysql_full_lifecycle: Docker/MariaDB not available");
        return;
    };

    // 1. Test connection (valid and invalid credentials).
    let cfg = db.config(None);
    let version = MysqlClient::test_connection(&cfg)
        .await
        .expect("test_connection should succeed");
    assert!(!version.is_empty());

    let mut bad_cfg = cfg.clone();
    bad_cfg.password = "wrong_password".to_string();
    let bad_res = MysqlClient::test_connection(&bad_cfg).await;
    assert!(bad_res.is_err());

    // 2. Connect & list databases.
    let (mut client, server_ver, databases) = MysqlClient::connect(cfg)
        .await
        .expect("connect should succeed");
    assert!(!server_ver.is_empty());
    assert!(databases.iter().any(|d| d.name == "testdb"));

    // 3. Switch database & create schema with diverse MySQL types + view + composite PK table.
    let initial_tables = client
        .use_database("testdb")
        .await
        .expect("use_database should succeed");
    assert!(initial_tables.is_empty());

    let pool = client.pool();
    let ddl_statements = [
        "CREATE TABLE items (
            id INT AUTO_INCREMENT PRIMARY KEY,
            flag BOOLEAN NOT NULL DEFAULT 1,
            big_signed BIGINT NOT NULL,
            big_unsigned BIGINT UNSIGNED NOT NULL,
            f_val FLOAT,
            d_val DOUBLE,
            dec_val DECIMAL(12, 4),
            name VARCHAR(100) NOT NULL,
            notes TEXT NULL,
            created_date DATE,
            created_at DATETIME,
            updated_at TIMESTAMP NULL,
            duration TIME,
            meta JSON,
            bits BIT(4),
            raw_bin VARBINARY(16)
        )",
        "INSERT INTO items (
            flag, big_signed, big_unsigned, f_val, d_val, dec_val, name, notes,
            created_date, created_at, updated_at, duration, meta, bits, raw_bin
        ) VALUES
        (1, -9000000000, 18446744073709551615, 1.5, 2.75, 12345.6789, 'alpha', 'first row',
         '2026-01-15', '2026-01-15 10:20:30', '2026-01-15 10:20:30', '01:02:03',
         '{\"k\":\"v\"}', b'1010', UNHEX('DEADBEEF')),
        (0, 42, 100, 3.25, 4.5, 0.0001, 'beta', NULL,
         '2026-02-20', '2026-02-20 12:00:00', NULL, '04:05:06',
         '[1,2,3]', b'0011', UNHEX('0102')),
        (1, 99, 200, 5.5, 6.5, 99.9900, 'gamma', 'third',
         '2026-03-10', '2026-03-10 08:30:00', NULL, '07:08:09',
         NULL, b'0001', NULL)",
        "CREATE VIEW items_view AS SELECT id, name FROM items",
        "CREATE TABLE composite_pk (
            tenant_id INT NOT NULL,
            item_id INT NOT NULL,
            label VARCHAR(64) NOT NULL,
            PRIMARY KEY (tenant_id, item_id)
        )",
        "INSERT INTO composite_pk VALUES (1, 10, 't1-i10'), (1, 20, 't1-i20')",
    ];

    for sql in ddl_statements {
        let res = execute_raw_query_on_pool(&pool, sql, Arc::new(AtomicU64::new(0)))
            .await
            .unwrap_or_else(|e| panic!("DDL failed ({sql}): {e}"));
        assert!(res.affected_rows.is_some());
    }

    // 4. List tables & views.
    let tables = client
        .list_tables("testdb")
        .await
        .expect("list_tables should succeed");
    assert!(tables
        .iter()
        .any(|t| t.name == "items" && t.kind == TableKind::Table));
    assert!(tables
        .iter()
        .any(|t| t.name == "items_view" && t.kind == TableKind::View));

    // 5. List columns & primary keys.
    let cols = client
        .list_columns("testdb", "items")
        .await
        .expect("list_columns should succeed");
    assert_eq!(cols[0].name, "id");
    assert!(cols[0].is_primary_key);
    assert!(!cols[0].nullable);
    assert!(cols.iter().any(|c| c.name == "notes" && c.nullable));

    // 6. Paginated fetch with all types, sorting, and filtering (1-based pages).
    let page1 = fetch_table_data_on_pool(
        &pool,
        "testdb",
        "items",
        1,
        2,
        Some(&SortSpec {
            column: "id".to_string(),
            direction: SortDirection::Asc,
        }),
        &[],
    )
    .await
    .expect("fetch page 1 should succeed");
    assert_eq!(page1.total_rows, Some(3));
    assert_eq!(page1.rows.len(), 2);
    assert_eq!(page1.primary_key_columns, vec!["id".to_string()]);
    assert_eq!(page1.rows[0][0], CellValue::Int(1));
    assert_eq!(page1.rows[0][2], CellValue::Int(-9_000_000_000));
    assert_eq!(
        page1.rows[0][3],
        CellValue::Text(u64::MAX.to_string())
    );
    assert_eq!(page1.rows[0][6], CellValue::Text("12345.6789".to_string()));
    assert_eq!(page1.rows[0][7], CellValue::Text("alpha".to_string()));
    assert_eq!(page1.rows[1][8], CellValue::Null);
    assert_eq!(
        page1.rows[0][15],
        CellValue::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF])
    );

    let page2 = fetch_table_data_on_pool(
        &pool,
        "testdb",
        "items",
        2,
        2,
        Some(&SortSpec {
            column: "id".to_string(),
            direction: SortDirection::Asc,
        }),
        &[],
    )
    .await
    .expect("fetch page 2 should succeed");
    assert_eq!(page2.total_rows, Some(3));
    assert_eq!(page2.rows.len(), 1);
    assert_eq!(page2.rows[0][7], CellValue::Text("gamma".to_string()));

    let filtered = fetch_table_data_on_pool(
        &pool,
        "testdb",
        "items",
        1,
        10,
        None,
        &[FilterSpec {
            column: "notes".to_string(),
            operator: FilterOperator::IsNull,
            value: String::new(),
        }],
    )
    .await
    .expect("filtered fetch should succeed");
    assert_eq!(filtered.total_rows, Some(1));
    assert_eq!(filtered.rows[0][7], CellValue::Text("beta".to_string()));

    // Multi-filter AND + ORDER BY DESC
    let multi_filtered = fetch_table_data_on_pool(
        &pool,
        "testdb",
        "items",
        1,
        10,
        Some(&SortSpec {
            column: "id".to_string(),
            direction: SortDirection::Desc,
        }),
        &[
            FilterSpec {
                column: "flag".to_string(),
                operator: FilterOperator::Eq,
                value: "1".to_string(),
            },
            FilterSpec {
                column: "notes".to_string(),
                operator: FilterOperator::IsNotNull,
                value: String::new(),
            },
        ],
    )
    .await
    .expect("multi-filter AND + ORDER BY should succeed");
    assert_eq!(multi_filtered.total_rows, Some(2));
    assert_eq!(multi_filtered.rows.len(), 2);
    assert_eq!(multi_filtered.rows[0][7], CellValue::Text("gamma".to_string()));
    assert_eq!(multi_filtered.rows[1][7], CellValue::Text("alpha".to_string()));

    // 7. Batched TableChanges (INSERT + UPDATE + DELETE) including SQL injection payload.
    let comp_data = fetch_table_data_on_pool(&pool, "testdb", "composite_pk", 1, 50, None, &[])
        .await
        .expect("fetch composite_pk should succeed");
    assert_eq!(
        comp_data.primary_key_columns,
        vec!["tenant_id".to_string(), "item_id".to_string()]
    );

    let injection_str = "'; DROP TABLE items; --".to_string();
    let changes = TableChanges {
        database: "testdb".to_string(),
        table: "composite_pk".to_string(),
        columns: comp_data.columns.clone(),
        primary_key_columns: comp_data.primary_key_columns.clone(),
        original_rows: comp_data.rows.clone(),
        edits: vec![PendingEdit {
            row_index: 0,
            col_index: 2,
            new_value: CellValue::Text(injection_str.clone()),
        }],
        inserts: vec![PendingInsert {
            values: vec![
                ("tenant_id".to_string(), CellValue::Int(2)),
                ("item_id".to_string(), CellValue::Int(30)),
                ("label".to_string(), CellValue::Text("t2-i30".to_string())),
            ],
        }],
        deletes: vec![PendingDelete { row_index: 1 }],
    };

    let affected = apply_table_changes_on_pool(&pool, &changes)
        .await
        .expect("apply_table_changes should succeed");
    assert_eq!(affected, 3);

    let after = fetch_table_data_on_pool(
        &pool,
        "testdb",
        "composite_pk",
        1,
        50,
        Some(&SortSpec {
            column: "tenant_id".to_string(),
            direction: SortDirection::Asc,
        }),
        &[],
    )
    .await
    .expect("fetch after changes");
    assert_eq!(after.total_rows, Some(2));
    assert_eq!(after.rows[0][2], CellValue::Text(injection_str));
    assert_eq!(after.rows[1][2], CellValue::Text("t2-i30".to_string()));

    // 8. Raw SQL queries: SHOW, DESCRIBE, 0-row SELECT, syntax error.
    let show_res = execute_raw_query_on_pool(&pool, "SHOW TABLES", Arc::new(AtomicU64::new(0)))
        .await
        .expect("SHOW TABLES should succeed");
    assert!(show_res.rows.len() >= 3);

    let empty_select = execute_raw_query_on_pool(
        &pool,
        "SELECT id, name FROM items WHERE id = -999",
        Arc::new(AtomicU64::new(0)),
    )
    .await
    .expect("0-row SELECT should succeed");
    assert!(empty_select.rows.is_empty());
    assert_eq!(empty_select.columns.len(), 2);

    let syntax_err = execute_raw_query_on_pool(
        &pool,
        "SELECT FROM WHERE",
        Arc::new(AtomicU64::new(0)),
    )
    .await;
    assert!(syntax_err.is_err());

    client.close().await;

    // 9. Worker channel test including query cancellation on SELECT SLEEP(10).
    let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel();
    let (event_tx, event_rx) = std_mpsc::channel();
    spawn_db_worker(&tokio::runtime::Handle::current(), cmd_rx, event_tx);

    cmd_tx
        .send(DbCommand::Connect {
            config: db.config(Some("testdb")),
        })
        .unwrap();
    let ev = event_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(matches!(ev, DbEvent::Connected { .. }));

    cmd_tx
        .send(DbCommand::ExecuteQuery {
            request_id: 99,
            sql: "SELECT SLEEP(10)".to_string(),
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    cmd_tx.send(DbCommand::CancelQuery { request_id: 99 }).unwrap();

    let cancel_ev = event_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        cancel_ev,
        DbEvent::QueryCancelled { request_id: 99 }
    ));

    cmd_tx.send(DbCommand::Shutdown).unwrap();
}
