pub mod mysql;
pub mod query;

use crate::db::mysql::{
    apply_table_changes_on_pool, execute_raw_query_on_pool, fetch_table_data_on_pool,
    list_columns_on_pool, list_tables_on_pool, MysqlClient,
};
use crate::models::{DbCommand, DbEvent};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender as StdSender;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Mutex;
use tokio::task::AbortHandle;

struct ActiveQuery {
    abort_handle: AbortHandle,
    connection_id: Arc<AtomicU64>,
}

/// Spawn the background database worker task on the provided Tokio runtime handle.
///
/// The UI sends `DbCommand` values through `tokio::sync::mpsc::UnboundedSender<DbCommand>`,
/// and drains `DbEvent` values non-blockingly from `std::sync::mpsc::Receiver<DbEvent>`.
pub fn spawn_db_worker(
    handle: &Handle,
    mut cmd_rx: UnboundedReceiver<DbCommand>,
    event_tx: StdSender<DbEvent>,
) {
    let worker_handle = handle.clone();
    handle.spawn(async move {
        let mut client: Option<MysqlClient> = None;
        let active_queries: Arc<Mutex<HashMap<u64, ActiveQuery>>> =
            Arc::new(Mutex::new(HashMap::new()));

        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                DbCommand::TestConnection { config } => {
                    let tx = event_tx.clone();
                    worker_handle.spawn(async move {
                        let res = MysqlClient::test_connection(&config).await;
                        let _ = tx.send(DbEvent::TestConnectionResult(res));
                    });
                }

                DbCommand::Connect { config } => {
                    if let Some(existing) = client.take() {
                        cancel_all_queries(&active_queries).await;
                        existing.close().await;
                    }
                    match MysqlClient::connect(config.clone()).await {
                        Ok((new_client, server_version, databases)) => {
                            client = Some(new_client);
                            let _ = event_tx.send(DbEvent::Connected {
                                config,
                                server_version,
                                databases,
                            });
                        }
                        Err(message) => {
                            let _ = event_tx.send(DbEvent::Error {
                                request_id: None,
                                message,
                            });
                        }
                    }
                }

                DbCommand::Disconnect => {
                    cancel_all_queries(&active_queries).await;
                    if let Some(existing) = client.take() {
                        existing.close().await;
                    }
                    let _ = event_tx.send(DbEvent::Disconnected);
                }

                DbCommand::ListDatabases => {
                    let Some(c) = client.as_ref() else {
                        send_not_connected(&event_tx, None);
                        continue;
                    };
                    match c.list_databases().await {
                        Ok(dbs) => {
                            let _ = event_tx.send(DbEvent::DatabasesLoaded(dbs));
                        }
                        Err(message) => {
                            let _ = event_tx.send(DbEvent::Error {
                                request_id: None,
                                message,
                            });
                        }
                    }
                }

                DbCommand::UseDatabase { database } => {
                    let Some(c) = client.as_mut() else {
                        send_not_connected(&event_tx, None);
                        continue;
                    };
                    match c.use_database(&database).await {
                        Ok(tables) => {
                            let _ = event_tx.send(DbEvent::DatabaseSwitched { database, tables });
                        }
                        Err(message) => {
                            let _ = event_tx.send(DbEvent::Error {
                                request_id: None,
                                message,
                            });
                        }
                    }
                }

                DbCommand::ListTables { database } => {
                    let Some(c) = client.as_ref() else {
                        send_not_connected(&event_tx, None);
                        continue;
                    };
                    let pool = c.pool();
                    let tx = event_tx.clone();
                    worker_handle.spawn(async move {
                        match list_tables_on_pool(&pool, &database).await {
                            Ok(tables) => {
                                let _ = tx.send(DbEvent::TablesLoaded { database, tables });
                            }
                            Err(message) => {
                                let _ = tx.send(DbEvent::Error {
                                    request_id: None,
                                    message,
                                });
                            }
                        }
                    });
                }

                DbCommand::ListColumns { database, table } => {
                    let Some(c) = client.as_ref() else {
                        send_not_connected(&event_tx, None);
                        continue;
                    };
                    let pool = c.pool();
                    let tx = event_tx.clone();
                    worker_handle.spawn(async move {
                        match list_columns_on_pool(&pool, &database, &table).await {
                            Ok(columns) => {
                                let _ = tx.send(DbEvent::ColumnsLoaded {
                                    database,
                                    table,
                                    columns,
                                });
                            }
                            Err(message) => {
                                let _ = tx.send(DbEvent::Error {
                                    request_id: None,
                                    message,
                                });
                            }
                        }
                    });
                }

                DbCommand::FetchTableData {
                    request_id,
                    database,
                    table,
                    page,
                    page_size,
                    sort,
                    filters,
                } => {
                    let Some(c) = client.as_ref() else {
                        send_not_connected(&event_tx, Some(request_id));
                        continue;
                    };
                    let pool = c.pool();
                    let tx = event_tx.clone();
                    worker_handle.spawn(async move {
                        match fetch_table_data_on_pool(
                            &pool,
                            &database,
                            &table,
                            page,
                            page_size,
                            sort.as_ref(),
                            &filters,
                        )
                        .await
                        {
                            Ok(result) => {
                                let _ = tx.send(DbEvent::TableDataLoaded {
                                    request_id,
                                    database,
                                    table,
                                    page,
                                    page_size,
                                    result,
                                });
                            }
                            Err(message) => {
                                let _ = tx.send(DbEvent::Error {
                                    request_id: Some(request_id),
                                    message,
                                });
                            }
                        }
                    });
                }

                DbCommand::ExecuteQuery { request_id, sql } => {
                    let Some(c) = client.as_ref() else {
                        send_not_connected(&event_tx, Some(request_id));
                        continue;
                    };
                    let pool = c.pool();
                    let tx = event_tx.clone();
                    let queries_map = Arc::clone(&active_queries);
                    let conn_id = Arc::new(AtomicU64::new(0));
                    let conn_id_for_task = Arc::clone(&conn_id);

                    let join_handle = worker_handle.spawn(async move {
                        let res = execute_raw_query_on_pool(&pool, &sql, conn_id_for_task).await;
                        queries_map.lock().await.remove(&request_id);
                        match res {
                            Ok(result) => {
                                let _ = tx.send(DbEvent::QueryResult { request_id, result });
                            }
                            Err(message) => {
                                let _ = tx.send(DbEvent::Error {
                                    request_id: Some(request_id),
                                    message,
                                });
                            }
                        }
                    });

                    active_queries.lock().await.insert(
                        request_id,
                        ActiveQuery {
                            abort_handle: join_handle.abort_handle(),
                            connection_id: conn_id,
                        },
                    );
                }

                DbCommand::CancelQuery { request_id } => {
                    let removed = active_queries.lock().await.remove(&request_id);
                    if let Some(active) = removed {
                        active.abort_handle.abort();
                        let kill_id = active.connection_id.load(Ordering::SeqCst);
                        if kill_id > 0 {
                            if let Some(c) = client.as_ref() {
                                let pool = c.pool();
                                worker_handle.spawn(async move {
                                    let _ = sqlx::raw_sql(&format!("KILL QUERY {kill_id}"))
                                        .execute(&pool)
                                        .await;
                                });
                            }
                        }
                        let _ = event_tx.send(DbEvent::QueryCancelled { request_id });
                    }
                }

                DbCommand::ApplyTableChanges {
                    request_id,
                    changes,
                } => {
                    let Some(c) = client.as_ref() else {
                        send_not_connected(&event_tx, Some(request_id));
                        continue;
                    };
                    let pool = c.pool();
                    let tx = event_tx.clone();
                    let database = changes.database.clone();
                    let table = changes.table.clone();
                    worker_handle.spawn(async move {
                        match apply_table_changes_on_pool(&pool, &changes).await {
                            Ok(affected_rows) => {
                                let _ = tx.send(DbEvent::ChangesApplied {
                                    request_id,
                                    database,
                                    table,
                                    affected_rows,
                                });
                            }
                            Err(message) => {
                                let _ = tx.send(DbEvent::Error {
                                    request_id: Some(request_id),
                                    message,
                                });
                            }
                        }
                    });
                }

                DbCommand::Shutdown => {
                    cancel_all_queries(&active_queries).await;
                    if let Some(existing) = client.take() {
                        existing.close().await;
                    }
                    break;
                }
            }
        }
    });
}

async fn cancel_all_queries(active_queries: &Arc<Mutex<HashMap<u64, ActiveQuery>>>) {
    let mut guard = active_queries.lock().await;
    for (_, active) in guard.drain() {
        active.abort_handle.abort();
    }
}

fn send_not_connected(tx: &StdSender<DbEvent>, request_id: Option<u64>) {
    let _ = tx.send(DbEvent::Error {
        request_id,
        message: "Not connected to any database".to_string(),
    });
}
