use crate::models::{CellValue, QueryResult};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

fn neutralize_csv_formula(value: &str) -> String {
    if matches!(
        value.as_bytes().first(),
        Some(b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r')
    ) {
        format!("'{value}")
    } else {
        value.to_string()
    }
}

fn cell_to_csv_field(cell: &CellValue) -> String {
    match cell {
        CellValue::Null => String::new(),
        CellValue::Text(s) => neutralize_csv_formula(s),
        other => other.to_display_string(),
    }
}

/// Export a `QueryResult` to a CSV string with proper RFC 4180 escaping
/// and formula-injection neutralization for text cells and headers.
pub fn to_csv_string(result: &QueryResult) -> Result<String, ExportError> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    let headers: Vec<String> = result
        .columns
        .iter()
        .map(|c| neutralize_csv_formula(&c.name))
        .collect();
    writer.write_record(&headers)?;

    for row in &result.rows {
        let fields: Vec<String> = row.iter().map(cell_to_csv_field).collect();
        writer.write_record(&fields)?;
    }

    let bytes = writer.into_inner().map_err(|e| ExportError::Io(e.into_error()))?;
    Ok(String::from_utf8(bytes)?)
}

/// Export a `QueryResult` to a CSV file at `path`.
pub fn export_csv_file(result: &QueryResult, path: &Path) -> Result<(), ExportError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let csv_data = to_csv_string(result)?;
    fs::write(path, csv_data)?;
    Ok(())
}

/// Export a `QueryResult` to a pretty-printed JSON array of objects.
pub fn to_json_string(result: &QueryResult) -> Result<String, ExportError> {
    let mut objects = Vec::with_capacity(result.rows.len());
    for row in &result.rows {
        let mut map = serde_json::Map::with_capacity(result.columns.len());
        for (idx, col) in result.columns.iter().enumerate() {
            let value = row
                .get(idx)
                .map(CellValue::to_json_value)
                .unwrap_or(serde_json::Value::Null);
            map.insert(col.name.clone(), value);
        }
        objects.push(serde_json::Value::Object(map));
    }
    Ok(serde_json::to_string_pretty(&objects)?)
}

/// Export a `QueryResult` to a JSON file at `path`.
pub fn export_json_file(result: &QueryResult, path: &Path) -> Result<(), ExportError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let json_data = to_json_string(result)?;
    fs::write(path, json_data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ColumnInfo;
    use uuid::Uuid;

    fn sample_result() -> QueryResult {
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
                    data_type: "VARCHAR".to_string(),
                    nullable: true,
                    is_primary_key: false,
                    default_value: None,
                },
                ColumnInfo {
                    name: "active".to_string(),
                    data_type: "TINYINT(1)".to_string(),
                    nullable: false,
                    is_primary_key: false,
                    default_value: None,
                },
                ColumnInfo {
                    name: "score".to_string(),
                    data_type: "DOUBLE".to_string(),
                    nullable: true,
                    is_primary_key: false,
                    default_value: None,
                },
                ColumnInfo {
                    name: "raw".to_string(),
                    data_type: "BLOB".to_string(),
                    nullable: true,
                    is_primary_key: false,
                    default_value: None,
                },
            ],
            rows: vec![
                vec![
                    CellValue::Int(1),
                    CellValue::Text("Alice, \"The Boss\"\nLine2".to_string()),
                    CellValue::Bool(true),
                    CellValue::Float(9.5),
                    CellValue::Bytes(vec![0xde, 0xad]),
                ],
                vec![
                    CellValue::Int(2),
                    CellValue::Null,
                    CellValue::Bool(false),
                    CellValue::Null,
                    CellValue::Null,
                ],
            ],
            total_rows: Some(2),
            affected_rows: None,
            execution_ms: 4,
            primary_key_columns: vec!["id".to_string()],
        }
    }

    #[test]
    fn csv_export_escapes_commas_quotes_and_newlines() {
        let result = sample_result();
        let csv_str = to_csv_string(&result).expect("csv export should succeed");
        let expected = "id,name,active,score,raw\n1,\"Alice, \"\"The Boss\"\"\nLine2\",true,9.5,0xdead\n2,,false,,\n";
        assert_eq!(csv_str, expected);
    }

    #[test]
    fn json_export_preserves_types_and_nulls() {
        let result = sample_result();
        let json_str = to_json_string(&result).expect("json export should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("valid json");
        let arr = parsed.as_array().expect("json array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[0]["name"], "Alice, \"The Boss\"\nLine2");
        assert_eq!(arr[0]["active"], true);
        assert_eq!(arr[0]["score"], 9.5);
        assert_eq!(arr[0]["raw"], "0xdead");
        assert_eq!(arr[1]["id"], 2);
        assert!(arr[1]["name"].is_null());
        assert_eq!(arr[1]["active"], false);
        assert!(arr[1]["score"].is_null());
    }

    #[test]
    fn export_files_write_to_disk() {
        let result = sample_result();
        let dir = std::env::temp_dir().join(format!("rust-dbviewer-export-{}", Uuid::new_v4()));
        let csv_path = dir.join("out.csv");
        let json_path = dir.join("out.json");

        export_csv_file(&result, &csv_path).expect("write csv");
        export_json_file(&result, &json_path).expect("write json");

        assert_eq!(fs::read_to_string(&csv_path).unwrap(), to_csv_string(&result).unwrap());
        assert_eq!(fs::read_to_string(&json_path).unwrap(), to_json_string(&result).unwrap());

        let _ = fs::remove_file(&csv_path);
        let _ = fs::remove_file(&json_path);
    }

    #[test]
    fn csv_export_neutralizes_formula_injection() {
        let result = QueryResult {
            columns: vec![
                ColumnInfo {
                    name: "=cmd".to_string(),
                    data_type: "VARCHAR".to_string(),
                    nullable: true,
                    is_primary_key: false,
                    default_value: None,
                },
                ColumnInfo {
                    name: "amount".to_string(),
                    data_type: "INT".to_string(),
                    nullable: false,
                    is_primary_key: false,
                    default_value: None,
                },
            ],
            rows: vec![
                vec![CellValue::Text("=1+1".to_string()), CellValue::Int(-42)],
                vec![CellValue::Text("+SUM(A1)".to_string()), CellValue::Int(10)],
                vec![CellValue::Text("-2+3".to_string()), CellValue::Int(0)],
                vec![CellValue::Text("@SUM(1)".to_string()), CellValue::Int(1)],
            ],
            total_rows: Some(4),
            affected_rows: None,
            execution_ms: 1,
            primary_key_columns: vec![],
        };

        let csv_str = to_csv_string(&result).expect("csv export should succeed");
        let expected = "'=cmd,amount\n'=1+1,-42\n'+SUM(A1),10\n'-2+3,0\n'@SUM(1),1\n";
        assert_eq!(csv_str, expected);
    }
}
