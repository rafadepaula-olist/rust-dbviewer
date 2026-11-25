use crate::models::ConnectionConfig;
use std::fs;
use std::io::Write;
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
/// Restricts file and directory permissions to owner-only on Unix systems.
pub fn save_connections_to(
    path: &Path,
    connections: &[ConnectionConfig],
) -> Result<(), StorageError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    let json = serde_json::to_string_pretty(connections)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(json.as_bytes())?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let mut file = fs::File::create(path)?;
        file.write_all(json.as_bytes())?;
    }
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

        let original = vec![c1.clone(), c2.clone()];
        save_connections_to(&path, &original).expect("save should succeed");

        // Plaintext passwords must never be written to disk.
        let raw_json = fs::read_to_string(&path).expect("read saved json");
        assert!(!raw_json.contains("secret"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let loaded = load_connections_from(&path).expect("load should succeed");
        let mut expected_c1 = c1;
        expected_c1.password.clear();
        assert_eq!(vec![expected_c1, c2], loaded);

        // Debug output must redact passwords.
        let debug_str = format!("{:?}", original[0]);
        assert!(!debug_str.contains("secret"));
        assert!(debug_str.contains("[REDACTED]"));

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
