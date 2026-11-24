use rust_dbviewer::db::spawn_db_worker;
use rust_dbviewer::models::{DbCommand, DbEvent};
use rust_dbviewer::ui::DbViewerApp;
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc as tokio_mpsc;

fn main() -> eframe::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start tokio runtime");

    let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel::<DbCommand>();
    let (event_tx, event_rx) = std_mpsc::channel::<DbEvent>();

    spawn_db_worker(runtime.handle(), cmd_rx, event_tx);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rust DBViewer — MySQL / MariaDB")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };

    let cmd_tx_shutdown = cmd_tx.clone();
    let result = eframe::run_native(
        "Rust DBViewer",
        native_options,
        Box::new(move |cc| Ok(Box::new(DbViewerApp::new(cc, cmd_tx, event_rx)))),
    );

    let _ = cmd_tx_shutdown.send(DbCommand::Shutdown);
    drop(runtime);

    result
}
