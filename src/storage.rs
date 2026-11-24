use crate::models::ConnectionConfig;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("failed to determine config directory")]
    NoConfigDir,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Default path for saved connections: `~/.config/rust-dbviewer/connections.json`.
pub fn default_connections_path() -> Result<PathBuf, StorageError> {
    let base = dirs::config_dir().ok_or(StorageError::NoConfigDir)?;
    Ok(base.join("rust-dbviewer").join("connections.json"))
}

/// Load saved connections from the default config path.
pub fn load_connections() -> Result<Vec<ConnectionConfig>, StorageError> {
    let path = default_connections_path()?;
    load_connections_from(&path)
}

/// Save connections to the default config path.
pub fn save_connections(connections: &[ConnectionConfig]) -> Result<(), StorageError> {
    let path = default_connections_path()?;
    save_connections_to(&path, connections)
}

/// Load saved connections from a specific file path.
/// Returns an empty list if the file does not exist yet.
pub fn load_connections_from(path: &Path) -> Result<Vec<ConnectionConfig>, StorageError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let connections = serde_json::from_str(&content)?;
    Ok(connections)
}

/// Save connections to a specific file path, creating parent directories if needed.
pub fn save_connections_to(
    path: &Path,
    connections: &[ConnectionConfig],
) -> Result<(), StorageError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(connections)?;
    fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_file_path(label: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("rust-dbviewer-test-{}-{}", label, Uuid::new_v4()))
            .join("connections.json")
    }

    #[test]
    fn load_missing_file_returns_empty_vec() {
        let path = temp_file_path("missing");
        let loaded = load_connections_from(&path).expect("missing file should return empty");
        assert!(loaded.is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = temp_file_path("roundtrip");
        let mut c1 = ConnectionConfig::new(
            "Local Dev",
            "127.0.0.1",
            3306,
            "root",
            "secret",
            Some("app_db".to_string()),
        );
        c1.use_ssl = true;
        c1.connect_timeout_secs = 15;
        c1.color = Some("#f59e0b".to_string());

        let c2 = ConnectionConfig::new("Staging", "10.0.0.5", 3307, "readonly", "", None);

        let original = vec![c1, c2];
        save_connections_to(&path, &original).expect("save should succeed");
        let loaded = load_connections_from(&path).expect("load should succeed");
        assert_eq!(original, loaded);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_invalid_json_returns_error() {
        let path = temp_file_path("invalid");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, "{ not valid json").unwrap();
        let err = load_connections_from(&path).unwrap_err();
        assert!(matches!(err, StorageError::Json(_)));

        let _ = fs::remove_file(&path);
    }
}
