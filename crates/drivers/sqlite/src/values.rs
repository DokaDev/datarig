//! Values and column types as the protocol carries them. SQLite types values, not columns: a
//! column's declared type gives its kind (by SQLite's affinity rules), and each value is written
//! by its own storage class.

use datarig_core::driver::{Cell, ColumnMeta, ValueKind};
use rusqlite::types::ValueRef;
use rusqlite::{Row, Statement};

/// The columns of a prepared statement that returns rows.
pub(crate) fn columns(stmt: &Statement<'_>) -> Vec<ColumnMeta> {
    stmt.columns()
        .map(|column| {
            let declared = column.decl_type().unwrap_or("").to_string();
            let kind = kind_of(&declared);
            let numeric = matches!(kind, ValueKind::Integer | ValueKind::Float | ValueKind::Decimal);
            ColumnMeta {
                name: column.name().to_string(),
                type_name: declared,
                numeric,
                json: false,
                kind,
                origin: None,
            }
        })
        .collect()
}

/// The kind a declared column type gives, by SQLite's affinity rules (`INT…`, `CHAR`/`CLOB`/
/// `TEXT`, `BLOB`, `REAL`/`FLOA`/`DOUB`, `NUMERIC`/`DECIMAL`); no declared type is `Other`.
fn kind_of(declared: &str) -> ValueKind {
    let t = declared.to_ascii_uppercase();
    if t.is_empty() {
        ValueKind::Other
    } else if t.contains("INT") {
        ValueKind::Integer
    } else if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        ValueKind::Text
    } else if t.contains("BLOB") {
        ValueKind::Bytes
    } else if t.contains("REAL") || t.contains("FLOA") || t.contains("DOUB") {
        ValueKind::Float
    } else if t.contains("BOOL") {
        ValueKind::Bool
    } else if t.contains("DATE") && t.contains("TIME") {
        ValueKind::Timestamp
    } else if t.contains("DATE") {
        ValueKind::Date
    } else if t.contains("TIME") {
        ValueKind::Time
    } else if t.contains("DECIMAL") || t.contains("NUMERIC") {
        ValueKind::Decimal
    } else {
        ValueKind::Other
    }
}

/// The `width` cells of a row, as text (`None` for NULL).
pub(crate) fn cells(row: &Row<'_>, width: usize) -> rusqlite::Result<Vec<Cell>> {
    (0..width).map(|i| row.get_ref(i).map(cell)).collect()
}

fn cell(value: ValueRef<'_>) -> Cell {
    match value {
        ValueRef::Null => None,
        ValueRef::Integer(n) => Some(n.to_string()),
        // Debug keeps a real's point (`1.0`), which Display drops.
        ValueRef::Real(f) => Some(format!("{f:?}")),
        ValueRef::Text(text) => Some(String::from_utf8_lossy(text).into_owned()),
        ValueRef::Blob(bytes) => Some(format!("\\x{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())),
    }
}
