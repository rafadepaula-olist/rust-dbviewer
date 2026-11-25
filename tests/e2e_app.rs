use rust_dbviewer::db::mysql::MysqlClient;
use rust_dbviewer::db::spawn_db_worker;
use rust_dbviewer::export::{export_csv_file, export_json_file, to_csv_string, to_json_string};
use rust_dbviewer::models::{
    CellValue, ConnectionConfig, DbCommand, DbEvent, FilterOperator, FilterSpec, SortDirection,
    SortSpec, TableKind,
};
use rust_dbviewer::storage::{load_connections_from, save_connections_to};
use rust_dbviewer::ui::{DbViewerApp, Tab};
use std::fs;
use std::process::Command;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;
use uuid::Uuid;

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
                "MARIADB_ROOT_PASSWORD=e2epass",
                "-e",
                "MARIADB_DATABASE=e2edb",
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
        let cfg = instance.config(Some("e2edb"));
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if MysqlClient::test_connection(&cfg).await.is_ok() {
                return Some(instance);
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        None
    }

    fn config(&self, database: Option<&str>) -> ConnectionConfig {
        let mut cfg = ConnectionConfig::new(
            "E2E MariaDB",
            "127.0.0.1",
            self.port,
            "root",
            "e2epass",
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

/// Wait until a predicate on `DbViewerApp` becomes true, draining events from the worker.
async fn wait_until<F>(app: &mut DbViewerApp, timeout: Duration, mut pred: F)
where
    F: FnMut(&DbViewerApp) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        app.drain_events();
        if pred(app) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    app.drain_events();
    assert!(
        pred(app),
        "timed out waiting for expected app state; toast={:?}",
        app.toast
    );
}

#[test]
fn e2e_storage_roundtrip() {
    let dir = std::env::temp_dir().join(format!("rust-dbviewer-e2e-storage-{}", Uuid::new_v4()));
    let path = dir.join("connections.json");

    let mut prod = ConnectionConfig::new(
        "Production RO",
        "db.internal",
        3306,
        "analyst",
        "s3cr3t",
        Some("analytics".to_string()),
    );
    prod.use_ssl = true;
    prod.connect_timeout_secs = 20;
    prod.color = Some("#fad666".to_string());

    let local = ConnectionConfig::new("Local", "127.0.0.1", 3307, "root", "", None);

    save_connections_to(&path, &[prod.clone(), local.clone()]).expect("save connections");
    let loaded = load_connections_from(&path).expect("load connections");
    let mut expected_prod = prod;
    expected_prod.password.clear();
    assert_eq!(loaded, vec![expected_prod, local]);

    let _ = fs::remove_file(&path);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_full_app_workflow_against_mariadb() {
    let Some(db) = EphemeralMariaDb::start().await else {
        eprintln!("Skipping e2e_full_app_workflow_against_mariadb: Docker/MariaDB unavailable");
        return;
    };

    // Start the real async DB worker and wire it to a headless DbViewerApp.
    let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel::<DbCommand>();
    let (event_tx, event_rx) = std_mpsc::channel::<DbEvent>();
    spawn_db_worker(&tokio::runtime::Handle::current(), cmd_rx, event_tx);

    let cfg = db.config(Some("e2edb"));
    let mut app = DbViewerApp::new_with_connections(cmd_tx.clone(), event_rx, vec![cfg.clone()]);

    // 1. Test connection (valid + invalid).
    app.connection_screen.testing = true;
    cmd_tx
        .send(DbCommand::TestConnection {
            config: cfg.clone(),
        })
        .unwrap();
    wait_until(&mut app, Duration::from_secs(10), |a| {
        !a.connection_screen.testing
    })
    .await;
    let (is_err, banner) = app
        .connection_screen
        .status_banner
        .as_ref()
        .expect("test banner");
    assert!(!is_err, "expected test connection ok, got: {banner}");

    let mut bad_cfg = cfg.clone();
    bad_cfg.password = "bad_password".to_string();
    app.connection_screen.testing = true;
    cmd_tx
        .send(DbCommand::TestConnection { config: bad_cfg })
        .unwrap();
    wait_until(&mut app, Duration::from_secs(10), |a| {
        !a.connection_screen.testing
    })
    .await;
    let (is_err, _) = app
        .connection_screen
        .status_banner
        .as_ref()
        .expect("error banner");
    assert!(*is_err, "bad password should fail test connection");

    // 2. Connect -> workspace initializes + auto-switches to default database `e2edb`.
    app.connection_screen.connecting = true;
    cmd_tx.send(DbCommand::Connect { config: cfg }).unwrap();
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.active_connection.is_some() && !a.loading_tables
    })
    .await;
    assert_eq!(app.current_database.as_deref(), Some("e2edb"));
    assert!(app.databases.iter().any(|d| d.name == "e2edb"));
    assert_eq!(app.tabs.len(), 1);
    assert!(matches!(app.tabs[0], Tab::Sql(_)));

    // 3. Seed schema via raw SQL through the worker: diverse types, composite PK, view.
    let setup_queries = [
        "CREATE TABLE products (
            id INT AUTO_INCREMENT PRIMARY KEY,
            sku VARCHAR(32) NOT NULL,
            price DECIMAL(10, 2) NOT NULL,
            in_stock TINYINT(1) NOT NULL DEFAULT 1,
            created_at DATETIME NOT NULL,
            notes TEXT NULL,
            payload BLOB NULL
        )",
        "INSERT INTO products (sku, price, in_stock, created_at, notes, payload) VALUES
            ('SKU-001', 19.99, 1, '2026-01-01 09:00:00', 'First, \"quoted\" item', UNHEX('CAFEBABE')),
            ('SKU-002', 49.50, 0, '2026-01-02 10:30:00', NULL, NULL),
            ('SKU-003', 120.00, 1, '2026-01-03 14:15:00', 'Line1\nLine2', UNHEX('010203')),
            ('SKU-004', 5.25, 1, '2026-01-04 16:45:00', 'Budget', NULL),
            ('SKU-005', 250.00, 1, '2026-01-05 18:00:00', 'Premium', UNHEX('FF'))",
        "CREATE VIEW active_products AS SELECT id, sku, price FROM products WHERE in_stock = 1",
        "CREATE TABLE order_items (
            order_id INT NOT NULL,
            line_no INT NOT NULL,
            qty INT NOT NULL,
            PRIMARY KEY (order_id, line_no)
        )",
        "INSERT INTO order_items VALUES (100, 1, 2), (100, 2, 5)",
    ];

    let sql_tab_id = app.tabs[0].id();
    for (idx, sql) in setup_queries.iter().enumerate() {
        let req_id = 1000 + idx as u64;
        if let Tab::Sql(s) = &mut app.tabs[0] {
            s.running_request_id = Some(req_id);
            s.error = None;
        }
        cmd_tx
            .send(DbCommand::ExecuteQuery {
                request_id: req_id,
                sql: (*sql).to_string(),
            })
            .unwrap();
        wait_until(&mut app, Duration::from_secs(10), |a| {
            matches!(&a.tabs[0], Tab::Sql(s) if !s.is_running())
        })
        .await;
        if let Tab::Sql(s) = &app.tabs[0] {
            assert!(s.error.is_none(), "setup SQL failed: {:?}", s.error);
        }
    }

    // 4. Introspect tables, views, and columns.
    app.loading_tables = true;
    cmd_tx
        .send(DbCommand::ListTables {
            database: "e2edb".to_string(),
        })
        .unwrap();
    wait_until(&mut app, Duration::from_secs(10), |a| !a.loading_tables).await;
    assert!(app
        .tables
        .iter()
        .any(|t| t.name == "products" && t.kind == TableKind::Table));
    assert!(app
        .tables
        .iter()
        .any(|t| t.name == "order_items" && t.kind == TableKind::Table));
    assert!(app
        .tables
        .iter()
        .any(|t| t.name == "active_products" && t.kind == TableKind::View));

    cmd_tx
        .send(DbCommand::ListColumns {
            database: "e2edb".to_string(),
            table: "order_items".to_string(),
        })
        .unwrap();
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.columns_cache
            .contains_key(&("e2edb".to_string(), "order_items".to_string()))
    })
    .await;
    let oi_cols = &app.columns_cache[&("e2edb".to_string(), "order_items".to_string())];
    let pk_names: Vec<&str> = oi_cols
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(pk_names, vec!["order_id", "line_no"]);

    // 5. Open `products` table tab, test pagination + sorting + filtering.
    let products_tab_id = app.open_table_tab("e2edb".to_string(), "products".to_string());
    assert_eq!(app.active_tab_id, Some(products_tab_id));
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.tabs
            .iter()
            .any(|t| matches!(t, Tab::Table { id, state } if *id == products_tab_id && !state.loading && state.result.is_some()))
    })
    .await;

    // Configure page_size=2, sort by price DESC, reload page 1 & page 2.
    {
        let grid = app
            .tabs
            .iter_mut()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == products_tab_id => Some(state),
                _ => None,
            })
            .unwrap();
        assert_eq!(grid.result.as_ref().unwrap().total_rows, Some(5));
        assert!(grid.can_edit());
        grid.page_size = 2;
        grid.page = 1;
        grid.sort = Some(SortSpec {
            column: "price".to_string(),
            direction: SortDirection::Desc,
        });
    }
    app.reload_table_tab(products_tab_id);
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.tabs
            .iter()
            .any(|t| matches!(t, Tab::Table { id, state } if *id == products_tab_id && !state.loading))
    })
    .await;

    {
        let grid = app
            .tabs
            .iter()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == products_tab_id => Some(state),
                _ => None,
            })
            .unwrap();
        assert_eq!(grid.total_pages(), 3); // 5 rows / 2 per page = 3 pages
        let res = grid.result.as_ref().unwrap();
        assert_eq!(res.rows.len(), 2);
        assert_eq!(res.rows[0][1], CellValue::Text("SKU-005".to_string()));
        assert_eq!(res.rows[1][1], CellValue::Text("SKU-003".to_string()));
    }

    // Filter with IS NULL on `notes`, then test multi-AND filters (`in_stock = 1 AND price >= 50`) + ORDER BY.
    {
        let grid = app
            .tabs
            .iter_mut()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == products_tab_id => Some(state),
                _ => None,
            })
            .unwrap();
        grid.page = 1;
        grid.page_size = 50;
        grid.active_filters = vec![FilterSpec {
            column: "notes".to_string(),
            operator: FilterOperator::IsNull,
            value: String::new(),
        }];
    }
    app.reload_table_tab(products_tab_id);
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.tabs
            .iter()
            .any(|t| matches!(t, Tab::Table { id, state } if *id == products_tab_id && !state.loading))
    })
    .await;

    {
        let grid = app
            .tabs
            .iter_mut()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == products_tab_id => Some(state),
                _ => None,
            })
            .unwrap();
        let res = grid.result.as_ref().unwrap();
        assert_eq!(res.total_rows, Some(1));
        assert_eq!(res.rows[0][1], CellValue::Text("SKU-002".to_string()));

        // Multi-AND filter via FilterRow + apply_filter_rows() combined with ORDER BY price ASC
        grid.filter_rows = vec![
            rust_dbviewer::ui::data_grid::FilterRow {
                column: "in_stock".to_string(),
                operator: FilterOperator::Eq,
                value: "1".to_string(),
            },
            rust_dbviewer::ui::data_grid::FilterRow {
                column: "price".to_string(),
                operator: FilterOperator::Gte,
                value: "50".to_string(),
            },
        ];
        grid.apply_filter_rows();
        assert_eq!(grid.active_filters.len(), 2);
        grid.sort = Some(SortSpec {
            column: "price".to_string(),
            direction: SortDirection::Asc,
        });
    }
    app.reload_table_tab(products_tab_id);
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.tabs
            .iter()
            .any(|t| matches!(t, Tab::Table { id, state } if *id == products_tab_id && !state.loading))
    })
    .await;

    {
        let grid = app
            .tabs
            .iter_mut()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == products_tab_id => Some(state),
                _ => None,
            })
            .unwrap();
        let res = grid.result.as_ref().unwrap();
        // Matches SKU-003 (120.00) and SKU-005 (250.00), sorted ASC by price
        assert_eq!(res.total_rows, Some(2));
        assert_eq!(res.rows[0][1], CellValue::Text("SKU-003".to_string()));
        assert_eq!(res.rows[1][1], CellValue::Text("SKU-005".to_string()));

        // Clear filter and reload all rows sorted by id ASC.
        grid.clear_filters();
        assert!(grid.active_filters.is_empty());
        grid.sort = Some(SortSpec {
            column: "id".to_string(),
            direction: SortDirection::Asc,
        });
    }
    app.reload_table_tab(products_tab_id);
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.tabs
            .iter()
            .any(|t| matches!(t, Tab::Table { id, state } if *id == products_tab_id && !state.loading))
    })
    .await;

    // 6. Export CSV & JSON from loaded table data and verify contents.
    {
        let grid = app
            .tabs
            .iter()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == products_tab_id => Some(state),
                _ => None,
            })
            .unwrap();
        let res = grid.result.as_ref().unwrap();

        let export_dir =
            std::env::temp_dir().join(format!("rust-dbviewer-e2e-export-{}", Uuid::new_v4()));
        let csv_path = export_dir.join("products.csv");
        let json_path = export_dir.join("products.json");

        export_csv_file(res, &csv_path).expect("export csv");
        export_json_file(res, &json_path).expect("export json");

        let csv_text = fs::read_to_string(&csv_path).unwrap();
        assert_eq!(csv_text, to_csv_string(res).unwrap());
        assert!(csv_text.contains("\"First, \"\"quoted\"\" item\""));
        assert!(csv_text.contains("0xcafebabe"));

        let json_text = fs::read_to_string(&json_path).unwrap();
        assert_eq!(json_text, to_json_string(res).unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 5);
        assert_eq!(arr[0]["sku"], "SKU-001");
        assert!(arr[1]["notes"].is_null());
        assert_eq!(arr[0]["payload"], "0xcafebabe");

        let _ = fs::remove_file(&csv_path);
        let _ = fs::remove_file(&json_path);
    }

    // 7. Inline UPDATE + INSERT + DELETE via DataGridState -> TableChanges -> DB worker -> auto-reload.
    let changes = {
        let grid = app
            .tabs
            .iter_mut()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == products_tab_id => Some(state),
                _ => None,
            })
            .unwrap();

        // Update row 0 price and notes
        grid.stage_edit(0, 2, CellValue::Text("21.50".to_string()));
        grid.stage_edit(0, 5, CellValue::Text("Updated note".to_string()));
        // Mark row 3 (SKU-004) for deletion
        grid.toggle_delete_row(3);
        // Add a new row (explicit values for NOT NULL columns)
        grid.add_empty_row();
        let ins = grid.pending_inserts.last_mut().unwrap();
        ins[1] = CellValue::Text("SKU-006".to_string());
        ins[2] = CellValue::Text("77.77".to_string());
        ins[3] = CellValue::Int(1);
        ins[4] = CellValue::Text("2026-01-06 12:00:00".to_string());
        ins[5] = CellValue::Text("Inserted via grid".to_string());

        let changes = grid.build_table_changes().expect("should build changes");
        grid.loading = true;
        grid.pending_request_id = Some(5000);
        changes
    };

    cmd_tx
        .send(DbCommand::ApplyTableChanges {
            request_id: 5000,
            changes,
        })
        .unwrap();

    // ChangesApplied triggers reload_table_tab automatically; wait until reload finishes.
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.tabs.iter().any(|t| {
            matches!(t, Tab::Table { id, state }
                if *id == products_tab_id
                && !state.loading
                && !state.has_pending_changes()
                && state.result.as_ref().is_some_and(|r| r.rows.iter().any(|row| row[1] == CellValue::Text("SKU-006".to_string()))))
        })
    })
    .await;

    {
        let grid = app
            .tabs
            .iter()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == products_tab_id => Some(state),
                _ => None,
            })
            .unwrap();
        let res = grid.result.as_ref().unwrap();
        // 5 original - 1 deleted + 1 inserted = 5 rows
        assert_eq!(res.total_rows, Some(5));
        assert_eq!(res.rows[0][2], CellValue::Text("21.50".to_string()));
        assert_eq!(res.rows[0][5], CellValue::Text("Updated note".to_string()));
        assert!(!res
            .rows
            .iter()
            .any(|r| r[1] == CellValue::Text("SKU-004".to_string())));
        assert!(res
            .rows
            .iter()
            .any(|r| r[1] == CellValue::Text("SKU-006".to_string())));
    }

    // 8. Composite PK inline edit + delete on `order_items`.
    let oi_tab_id = app.open_table_tab("e2edb".to_string(), "order_items".to_string());
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.tabs
            .iter()
            .any(|t| matches!(t, Tab::Table { id, state } if *id == oi_tab_id && !state.loading && state.result.is_some()))
    })
    .await;

    let oi_changes = {
        let grid = app
            .tabs
            .iter_mut()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == oi_tab_id => Some(state),
                _ => None,
            })
            .unwrap();
        grid.stage_edit(0, 2, CellValue::Int(99));
        grid.toggle_delete_row(1);
        let c = grid.build_table_changes().expect("composite pk changes");
        grid.loading = true;
        grid.pending_request_id = Some(5001);
        c
    };

    cmd_tx
        .send(DbCommand::ApplyTableChanges {
            request_id: 5001,
            changes: oi_changes,
        })
        .unwrap();
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.tabs.iter().any(|t| {
            matches!(t, Tab::Table { id, state }
                if *id == oi_tab_id
                && !state.loading
                && state.result.as_ref().is_some_and(|r| r.total_rows == Some(1)))
        })
    })
    .await;

    {
        let grid = app
            .tabs
            .iter()
            .find_map(|t| match t {
                Tab::Table { id, state } if *id == oi_tab_id => Some(state),
                _ => None,
            })
            .unwrap();
        assert_eq!(grid.result.as_ref().unwrap().rows[0][2], CellValue::Int(99));
    }

    // 9. Raw SQL execution: SELECT + client-side column sorting + syntax error.
    app.active_tab_id = Some(sql_tab_id);
    if let Tab::Sql(s) = &mut app.tabs[0] {
        s.running_request_id = Some(6000);
        s.error = None;
    }
    cmd_tx
        .send(DbCommand::ExecuteQuery {
            request_id: 6000,
            sql: "SELECT id, sku, notes FROM products ORDER BY id ASC".to_string(),
        })
        .unwrap();
    wait_until(&mut app, Duration::from_secs(10), |a| {
        matches!(&a.tabs[0], Tab::Sql(s) if !s.is_running())
    })
    .await;
    if let Tab::Sql(s) = &mut app.tabs[0] {
        let res = s.result.as_ref().expect("select result");
        assert_eq!(res.rows.len(), 5);

        // Client-side sort by `id` DESC
        s.toggle_sort("id".to_string()); // ASC
        assert_eq!(s.sort.as_ref().map(|x| x.direction), Some(SortDirection::Asc));
        assert_eq!(s.sorted_row_indices[0], 0);

        s.toggle_sort("id".to_string()); // DESC
        assert_eq!(s.sort.as_ref().map(|x| x.direction), Some(SortDirection::Desc));
        assert_eq!(s.sorted_row_indices[0], 4);

        // Client-side sort by `notes` ASC puts NULL row (SKU-002, index 1) last
        s.toggle_sort("notes".to_string());
        assert_eq!(*s.sorted_row_indices.last().unwrap(), 1);
    }

    // Syntax error
    if let Tab::Sql(s) = &mut app.tabs[0] {
        s.running_request_id = Some(6001);
        s.error = None;
    }
    cmd_tx
        .send(DbCommand::ExecuteQuery {
            request_id: 6001,
            sql: "SELEC BROKEN SYNTAX".to_string(),
        })
        .unwrap();
    wait_until(&mut app, Duration::from_secs(10), |a| {
        matches!(&a.tabs[0], Tab::Sql(s) if !s.is_running())
    })
    .await;
    if let Tab::Sql(s) = &app.tabs[0] {
        assert!(s.error.is_some(), "syntax error should populate tab error");
    }

    // 10. Tab management & clean disconnect.
    app.close_tab(oi_tab_id);
    assert_eq!(app.tabs.len(), 2);

    cmd_tx.send(DbCommand::Disconnect).unwrap();
    wait_until(&mut app, Duration::from_secs(10), |a| {
        a.active_connection.is_none()
    })
    .await;
    assert!(app.tabs.is_empty());
    assert!(app.databases.is_empty());

    cmd_tx.send(DbCommand::Shutdown).unwrap();
}
