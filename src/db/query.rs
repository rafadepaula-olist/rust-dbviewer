use crate::models::{CellValue, FilterSpec, SortSpec, TableChanges};
use std::collections::{BTreeMap, HashSet};

/// A parameterized SQL statement with ordered bind parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltQuery {
    pub sql: String,
    pub params: Vec<CellValue>,
}

/// Escape a MySQL/MariaDB identifier by wrapping it in backticks and doubling internal backticks.
pub fn escape_ident(ident: &str) -> String {
    if ident == "*" {
        return "*".to_string();
    }
    let escaped = ident.replace('`', "``");
    format!("`{escaped}`")
}

/// Format a fully qualified `` `database`.`table` `` identifier.
pub fn qualified_table(database: &str, table: &str) -> String {
    format!("{}.{}", escape_ident(database), escape_ident(table))
}

/// Build the paginated `SELECT` query and matching `SELECT COUNT(*)` query for a table.
///
/// `page` is 1-based (`page = 1` returns the first `page_size` rows; `page = 0` is clamped to 1).
pub fn build_table_data_queries(
    database: &str,
    table: &str,
    page: u64,
    page_size: u64,
    sort: Option<&SortSpec>,
    filters: &[FilterSpec],
) -> (BuiltQuery, BuiltQuery) {
    let target = qualified_table(database, table);
    let mut conditions: Vec<String> = Vec::new();
    let mut filter_params: Vec<CellValue> = Vec::new();

    for f in filters {
        if f.column.trim().is_empty() {
            continue;
        }
        let col = escape_ident(&f.column);
        let op = f.operator.as_sql();
        if f.operator.takes_value() {
            conditions.push(format!("{col} {op} ?"));
            filter_params.push(CellValue::Text(f.value.clone()));
        } else {
            conditions.push(format!("{col} {op}"));
        }
    }

    let where_sql = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let order_sql = match sort {
        Some(s) if !s.column.is_empty() => {
            format!(" ORDER BY {} {}", escape_ident(&s.column), s.direction.as_sql())
        }
        _ => String::new(),
    };

    let limit = page_size.max(1);
    let offset = page.saturating_sub(1).saturating_mul(limit);

    let mut select_params = filter_params.clone();
    select_params.push(CellValue::Int(i64::try_from(limit).unwrap_or(i64::MAX)));
    select_params.push(CellValue::Int(i64::try_from(offset).unwrap_or(i64::MAX)));

    let select_query = BuiltQuery {
        sql: format!("SELECT * FROM {target}{where_sql}{order_sql} LIMIT ? OFFSET ?"),
        params: select_params,
    };

    let count_query = BuiltQuery {
        sql: format!("SELECT COUNT(*) AS total FROM {target}{where_sql}"),
        params: filter_params,
    };

    (select_query, count_query)
}

/// Build parameterized `INSERT` statements for all pending inserts.
pub fn build_insert_queries(changes: &TableChanges) -> Result<Vec<BuiltQuery>, String> {
    let target = qualified_table(&changes.database, &changes.table);
    let mut queries = Vec::with_capacity(changes.inserts.len());

    for insert in &changes.inserts {
        if insert.values.is_empty() {
            queries.push(BuiltQuery {
                sql: format!("INSERT INTO {target} () VALUES ()"),
                params: Vec::new(),
            });
            continue;
        }

        let cols: Vec<String> = insert.values.iter().map(|(c, _)| escape_ident(c)).collect();
        let placeholders = vec!["?"; insert.values.len()].join(", ");
        let params: Vec<CellValue> = insert.values.iter().map(|(_, v)| v.clone()).collect();

        queries.push(BuiltQuery {
            sql: format!(
                "INSERT INTO {target} ({}) VALUES ({placeholders})",
                cols.join(", ")
            ),
            params,
        });
    }

    Ok(queries)
}

/// Build parameterized `UPDATE` statements grouped by row, using original primary key values.
pub fn build_update_queries(changes: &TableChanges) -> Result<Vec<BuiltQuery>, String> {
    if changes.edits.is_empty() {
        return Ok(Vec::new());
    }
    if changes.primary_key_columns.is_empty() {
        return Err("Cannot update rows: table has no primary key".to_string());
    }

    let pk_indices = resolve_pk_indices(changes)?;
    let deleted_rows: HashSet<usize> = changes.deletes.iter().map(|d| d.row_index).collect();

    // Group edits by row_index, keeping the latest edit for any given (row_index, col_index).
    let mut edits_by_row: BTreeMap<usize, BTreeMap<usize, CellValue>> = BTreeMap::new();
    for edit in &changes.edits {
        if deleted_rows.contains(&edit.row_index) {
            continue;
        }
        if edit.row_index >= changes.original_rows.len() {
            return Err(format!("Edit row index {} out of bounds", edit.row_index));
        }
        if edit.col_index >= changes.columns.len() {
            return Err(format!("Edit column index {} out of bounds", edit.col_index));
        }
        edits_by_row
            .entry(edit.row_index)
            .or_default()
            .insert(edit.col_index, edit.new_value.clone());
    }

    let target = qualified_table(&changes.database, &changes.table);
    let mut queries = Vec::with_capacity(edits_by_row.len());

    for (row_idx, col_edits) in edits_by_row {
        let original_row = &changes.original_rows[row_idx];
        let mut set_clauses = Vec::with_capacity(col_edits.len());
        let mut params = Vec::with_capacity(col_edits.len() + pk_indices.len());

        for (col_idx, new_val) in col_edits {
            let col_name = &changes.columns[col_idx].name;
            set_clauses.push(format!("{} = ?", escape_ident(col_name)));
            params.push(new_val);
        }

        let (where_clause, pk_params) = build_pk_where(&pk_indices, original_row)?;
        params.extend(pk_params);

        queries.push(BuiltQuery {
            sql: format!(
                "UPDATE {target} SET {} WHERE {where_clause}",
                set_clauses.join(", ")
            ),
            params,
        });
    }

    Ok(queries)
}

/// Build parameterized `DELETE` statements using original primary key values.
pub fn build_delete_queries(changes: &TableChanges) -> Result<Vec<BuiltQuery>, String> {
    if changes.deletes.is_empty() {
        return Ok(Vec::new());
    }
    if changes.primary_key_columns.is_empty() {
        return Err("Cannot delete rows: table has no primary key".to_string());
    }

    let pk_indices = resolve_pk_indices(changes)?;
    let target = qualified_table(&changes.database, &changes.table);
    let mut seen = HashSet::new();
    let mut queries = Vec::with_capacity(changes.deletes.len());

    for del in &changes.deletes {
        if !seen.insert(del.row_index) {
            continue;
        }
        let original_row = changes
            .original_rows
            .get(del.row_index)
            .ok_or_else(|| format!("Delete row index {} out of bounds", del.row_index))?;

        let (where_clause, pk_params) = build_pk_where(&pk_indices, original_row)?;
        queries.push(BuiltQuery {
            sql: format!("DELETE FROM {target} WHERE {where_clause}"),
            params: pk_params,
        });
    }

    Ok(queries)
}

fn resolve_pk_indices(changes: &TableChanges) -> Result<Vec<(String, usize)>, String> {
    let mut indices = Vec::with_capacity(changes.primary_key_columns.len());
    for pk_col in &changes.primary_key_columns {
        let pos = changes
            .columns
            .iter()
            .position(|c| &c.name == pk_col)
            .ok_or_else(|| format!("Primary key column '{pk_col}' not found in table columns"))?;
        indices.push((pk_col.clone(), pos));
    }
    Ok(indices)
}

fn build_pk_where(
    pk_indices: &[(String, usize)],
    original_row: &[CellValue],
) -> Result<(String, Vec<CellValue>), String> {
    let mut clauses = Vec::with_capacity(pk_indices.len());
    let mut params = Vec::with_capacity(pk_indices.len());

    for (pk_name, col_idx) in pk_indices {
        let val = original_row
            .get(*col_idx)
            .ok_or_else(|| format!("Primary key column '{pk_name}' missing from row data"))?;
        if val.is_null() {
            clauses.push(format!("{} IS NULL", escape_ident(pk_name)));
        } else {
            clauses.push(format!("{} = ?", escape_ident(pk_name)));
            params.push(val.clone());
        }
    }

    Ok((clauses.join(" AND "), params))
}

/// Detect whether a raw SQL statement is expected to return a result set (rows/columns).
pub fn is_result_set_query(sql: &str) -> bool {
    let first_word = strip_leading_sql_comments(sql)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('(')
        .to_ascii_uppercase();

    matches!(
        first_word.as_str(),
        "SELECT"
            | "SHOW"
            | "DESCRIBE"
            | "DESC"
            | "EXPLAIN"
            | "WITH"
            | "TABLE"
            | "VALUES"
            | "HELP"
    )
}

fn strip_leading_sql_comments(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start();
        if let Some(rest) = sql.strip_prefix("--") {
            sql = match rest.find('\n') {
                Some(idx) => &rest[idx + 1..],
                None => "",
            };
        } else if let Some(rest) = sql.strip_prefix('#') {
            sql = match rest.find('\n') {
                Some(idx) => &rest[idx + 1..],
                None => "",
            };
        } else if let Some(rest) = sql.strip_prefix("/*") {
            sql = match rest.find("*/") {
                Some(idx) => &rest[idx + 2..],
                None => "",
            };
        } else {
            return sql;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ColumnInfo, FilterOperator, PendingDelete, PendingEdit, PendingInsert, SortDirection,
    };

    fn sample_columns() -> Vec<ColumnInfo> {
        vec![
            ColumnInfo {
                name: "id".to_string(),
                data_type: "int".to_string(),
                nullable: false,
                is_primary_key: true,
                default_value: None,
            },
            ColumnInfo {
                name: "name".to_string(),
                data_type: "varchar(255)".to_string(),
                nullable: false,
                is_primary_key: false,
                default_value: None,
            },
            ColumnInfo {
                name: "note".to_string(),
                data_type: "text".to_string(),
                nullable: true,
                is_primary_key: false,
                default_value: None,
            },
        ]
    }

    #[test]
    fn test_escape_ident() {
        assert_eq!(escape_ident("users"), "`users`");
        assert_eq!(escape_ident("user`s"), "`user``s`");
        assert_eq!(escape_ident("a``b"), "`a````b`");
        assert_eq!(escape_ident("*"), "*");
        assert_eq!(qualified_table("my`db", "my`table"), "`my``db`.`my``table`");
    }

    #[test]
    fn test_build_table_data_queries_basic_pagination() {
        let (sel1, cnt) = build_table_data_queries("app", "users", 1, 50, None, &[]);
        assert_eq!(sel1.sql, "SELECT * FROM `app`.`users` LIMIT ? OFFSET ?");
        assert_eq!(sel1.params, vec![CellValue::Int(50), CellValue::Int(0)]);
        assert_eq!(cnt.sql, "SELECT COUNT(*) AS total FROM `app`.`users`");
        assert!(cnt.params.is_empty());

        let (sel3, _) = build_table_data_queries("app", "users", 3, 50, None, &[]);
        assert_eq!(sel3.params, vec![CellValue::Int(50), CellValue::Int(100)]);
    }

    #[test]
    fn test_build_table_data_queries_with_filter_and_sort() {
        let sort = SortSpec {
            column: "created_at".to_string(),
            direction: SortDirection::Desc,
        };
        let filters = vec![FilterSpec {
            column: "email".to_string(),
            operator: FilterOperator::Like,
            value: "%@example.com".to_string(),
        }];
        let (sel, cnt) =
            build_table_data_queries("app", "users", 0, 100, Some(&sort), &filters);
        assert_eq!(
            sel.sql,
            "SELECT * FROM `app`.`users` WHERE `email` LIKE ? ORDER BY `created_at` DESC LIMIT ? OFFSET ?"
        );
        assert_eq!(
            sel.params,
            vec![
                CellValue::Text("%@example.com".to_string()),
                CellValue::Int(100),
                CellValue::Int(0),
            ]
        );
        assert_eq!(
            cnt.sql,
            "SELECT COUNT(*) AS total FROM `app`.`users` WHERE `email` LIKE ?"
        );
        assert_eq!(cnt.params, vec![CellValue::Text("%@example.com".to_string())]);
    }

    #[test]
    fn test_build_table_data_queries_multi_filter_and_sort() {
        let sort = SortSpec {
            column: "id".to_string(),
            direction: SortDirection::Desc,
        };
        let filters = vec![
            FilterSpec {
                column: "plataforma".to_string(),
                operator: FilterOperator::Eq,
                value: "205".to_string(),
            },
            FilterSpec {
                column: "idEmpresa".to_string(),
                operator: FilterOperator::Eq,
                value: "450683224".to_string(),
            },
            FilterSpec {
                column: "deleted_at".to_string(),
                operator: FilterOperator::IsNull,
                value: String::new(),
            },
        ];
        let (sel, cnt) =
            build_table_data_queries("app", "orders", 2, 50, Some(&sort), &filters);
        assert_eq!(
            sel.sql,
            "SELECT * FROM `app`.`orders` WHERE `plataforma` = ? AND `idEmpresa` = ? AND `deleted_at` IS NULL ORDER BY `id` DESC LIMIT ? OFFSET ?"
        );
        assert_eq!(
            sel.params,
            vec![
                CellValue::Text("205".to_string()),
                CellValue::Text("450683224".to_string()),
                CellValue::Int(50),
                CellValue::Int(50),
            ]
        );
        assert_eq!(
            cnt.sql,
            "SELECT COUNT(*) AS total FROM `app`.`orders` WHERE `plataforma` = ? AND `idEmpresa` = ? AND `deleted_at` IS NULL"
        );
        assert_eq!(
            cnt.params,
            vec![
                CellValue::Text("205".to_string()),
                CellValue::Text("450683224".to_string()),
            ]
        );
    }

    #[test]
    fn test_build_table_data_queries_is_null_filter() {
        let filters = vec![FilterSpec {
            column: "deleted_at".to_string(),
            operator: FilterOperator::IsNull,
            value: "ignored".to_string(),
        }];
        let (sel, cnt) = build_table_data_queries("app", "users", 0, 25, None, &filters);
        assert_eq!(
            sel.sql,
            "SELECT * FROM `app`.`users` WHERE `deleted_at` IS NULL LIMIT ? OFFSET ?"
        );
        assert_eq!(sel.params, vec![CellValue::Int(25), CellValue::Int(0)]);
        assert_eq!(
            cnt.sql,
            "SELECT COUNT(*) AS total FROM `app`.`users` WHERE `deleted_at` IS NULL"
        );
        assert!(cnt.params.is_empty());
    }

    #[test]
    fn test_build_insert_queries() {
        let changes = TableChanges {
            database: "app".to_string(),
            table: "users".to_string(),
            columns: sample_columns(),
            primary_key_columns: vec!["id".to_string()],
            original_rows: vec![],
            edits: vec![],
            inserts: vec![
                PendingInsert {
                    values: vec![
                        ("name".to_string(), CellValue::Text("Alice".to_string())),
                        ("note".to_string(), CellValue::Null),
                    ],
                },
                PendingInsert { values: vec![] },
            ],
            deletes: vec![],
        };

        let queries = build_insert_queries(&changes).unwrap();
        assert_eq!(queries.len(), 2);
        assert_eq!(
            queries[0].sql,
            "INSERT INTO `app`.`users` (`name`, `note`) VALUES (?, ?)"
        );
        assert_eq!(
            queries[0].params,
            vec![CellValue::Text("Alice".to_string()), CellValue::Null]
        );
        assert_eq!(queries[1].sql, "INSERT INTO `app`.`users` () VALUES ()");
    }

    #[test]
    fn test_build_update_queries_grouped_and_composite_pk() {
        let changes = TableChanges {
            database: "app".to_string(),
            table: "items".to_string(),
            columns: sample_columns(),
            primary_key_columns: vec!["id".to_string(), "name".to_string()],
            original_rows: vec![vec![
                CellValue::Int(7),
                CellValue::Text("old".to_string()),
                CellValue::Null,
            ]],
            edits: vec![
                PendingEdit {
                    row_index: 0,
                    col_index: 1,
                    new_value: CellValue::Text("new".to_string()),
                },
                PendingEdit {
                    row_index: 0,
                    col_index: 2,
                    new_value: CellValue::Text("hello".to_string()),
                },
            ],
            inserts: vec![],
            deletes: vec![],
        };

        let queries = build_update_queries(&changes).unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(
            queries[0].sql,
            "UPDATE `app`.`items` SET `name` = ?, `note` = ? WHERE `id` = ? AND `name` = ?"
        );
        assert_eq!(
            queries[0].params,
            vec![
                CellValue::Text("new".to_string()),
                CellValue::Text("hello".to_string()),
                CellValue::Int(7),
                CellValue::Text("old".to_string()),
            ]
        );
    }

    #[test]
    fn test_build_update_requires_primary_key() {
        let changes = TableChanges {
            database: "app".to_string(),
            table: "logs".to_string(),
            columns: sample_columns(),
            primary_key_columns: vec![],
            original_rows: vec![vec![CellValue::Int(1), CellValue::Text("a".into()), CellValue::Null]],
            edits: vec![PendingEdit {
                row_index: 0,
                col_index: 1,
                new_value: CellValue::Text("b".into()),
            }],
            inserts: vec![],
            deletes: vec![],
        };
        assert!(build_update_queries(&changes).is_err());
    }

    #[test]
    fn test_build_delete_queries_deduplicated() {
        let changes = TableChanges {
            database: "app".to_string(),
            table: "users".to_string(),
            columns: sample_columns(),
            primary_key_columns: vec!["id".to_string()],
            original_rows: vec![
                vec![CellValue::Int(10), CellValue::Text("a".into()), CellValue::Null],
                vec![CellValue::Int(20), CellValue::Text("b".into()), CellValue::Null],
            ],
            edits: vec![],
            inserts: vec![],
            deletes: vec![
                PendingDelete { row_index: 1 },
                PendingDelete { row_index: 1 },
            ],
        };

        let queries = build_delete_queries(&changes).unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].sql, "DELETE FROM `app`.`users` WHERE `id` = ?");
        assert_eq!(queries[0].params, vec![CellValue::Int(20)]);
    }

    #[test]
    fn test_is_result_set_query() {
        assert!(is_result_set_query("  SELECT * FROM users"));
        assert!(is_result_set_query("-- comment\nSHOW TABLES;"));
        assert!(is_result_set_query("/* block */ DESCRIBE users"));
        assert!(is_result_set_query("WITH cte AS (SELECT 1) SELECT * FROM cte"));
        assert!(!is_result_set_query("INSERT INTO users VALUES (1)"));
        assert!(!is_result_set_query("UPDATE users SET name = 'a'"));
        assert!(!is_result_set_query("DELETE FROM users WHERE id = 1"));
    }
}
