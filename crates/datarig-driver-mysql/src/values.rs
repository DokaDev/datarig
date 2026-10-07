//! Result columns and values as the app shows them. Results come in MySQL's text protocol, so
//! a value is the text the server writes (exact decimals, unsigned 64-bit numbers, zero dates,
//! times past 24 hours, timestamps in the session's time zone), shown as it is; only what is not
//! text is written out here: binary data (`BINARY`, `VARBINARY`, `BLOB`, `GEOMETRY`, `VECTOR`)
//! as `0x` and its bytes in hex, a `BIT` value as `b'0101'` (as many digits as the column has
//! bits).

use datarig_core::driver::{ColumnMeta, ColumnOrigin, ValueKind};
use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::{Column, Value};

/// The character set number of binary data (`binary`): a string column of it holds bytes.
const BINARY: u16 = 63;

/// How many bytes a character of the session's result character set (`utf8mb4`) takes at most:
/// the server gives the length of a text column in bytes.
const MAX_CHAR_BYTES: u32 = 4;

/// The column as the app sees it: its name, its type as MySQL names it (`bigint unsigned`,
/// `varchar`, `json`), its kind, and the table column it comes from when it is one (the
/// database, the table and the column's own names, not the result's aliases).
pub(crate) fn column_meta(c: &Column) -> ColumnMeta {
    let table = c.org_table_str();
    let column = c.org_name_str();
    let origin = (!table.is_empty() && !column.is_empty()).then(|| ColumnOrigin::Named {
        schema: c.schema_str().into_owned(),
        table: table.into_owned(),
        column: column.into_owned(),
    });
    ColumnMeta::new(c.name_str().into_owned(), type_name(c), value_kind(c), origin)
}

fn binary(c: &Column) -> bool {
    c.character_set() == BINARY
}

/// The type's name as MySQL writes it, lower case.
pub(crate) fn type_name(c: &Column) -> String {
    use ColumnType::*;
    let flags = c.flags();
    let unsigned = flags.contains(ColumnFlags::UNSIGNED_FLAG);
    let int = |name: &str| if unsigned { format!("{name} unsigned") } else { name.to_string() };
    match c.column_type() {
        MYSQL_TYPE_TINY if c.column_length() == 1 => int("tinyint(1)"),
        MYSQL_TYPE_TINY => int("tinyint"),
        MYSQL_TYPE_SHORT => int("smallint"),
        MYSQL_TYPE_INT24 => int("mediumint"),
        MYSQL_TYPE_LONG => int("int"),
        MYSQL_TYPE_LONGLONG => int("bigint"),
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => int("decimal"),
        MYSQL_TYPE_FLOAT => int("float"),
        MYSQL_TYPE_DOUBLE => int("double"),
        MYSQL_TYPE_BIT => "bit".to_string(),
        MYSQL_TYPE_YEAR => "year".to_string(),
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date".to_string(),
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time".to_string(),
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => "datetime".to_string(),
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => "timestamp".to_string(),
        MYSQL_TYPE_JSON => "json".to_string(),
        MYSQL_TYPE_GEOMETRY => "geometry".to_string(),
        MYSQL_TYPE_VECTOR => "vector".to_string(),
        MYSQL_TYPE_NULL => "null".to_string(),
        MYSQL_TYPE_ENUM => "enum".to_string(),
        MYSQL_TYPE_SET => "set".to_string(),
        // ENUM and SET columns come as strings with a flag.
        _ if flags.contains(ColumnFlags::ENUM_FLAG) => "enum".to_string(),
        _ if flags.contains(ColumnFlags::SET_FLAG) => "set".to_string(),
        MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_VARCHAR if binary(c) => "varbinary".to_string(),
        MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_VARCHAR => "varchar".to_string(),
        MYSQL_TYPE_STRING if binary(c) => "binary".to_string(),
        MYSQL_TYPE_STRING => "char".to_string(),
        MYSQL_TYPE_TINY_BLOB | MYSQL_TYPE_BLOB | MYSQL_TYPE_MEDIUM_BLOB | MYSQL_TYPE_LONG_BLOB => {
            // The server sends every one of them as `BLOB`, with its length.
            let bytes = if binary(c) { c.column_length() } else { c.column_length() / MAX_CHAR_BYTES };
            let size = match bytes {
                0..=255 => "tiny",
                256..=65_535 => "",
                65_536..=16_777_215 => "medium",
                _ => "long",
            };
            format!("{size}{}", if binary(c) { "blob" } else { "text" })
        }
        MYSQL_TYPE_TYPED_ARRAY | MYSQL_TYPE_UNKNOWN => "unknown".to_string(),
    }
}

/// What kind of value the column holds. `TINYINT(1)` is a number (MySQL has no boolean type, and
/// such a column may hold 2), `YEAR` a number, `TIME` a span of time (it may be negative or over
/// 24 hours), `TIMESTAMP` a point in time (shown in the session's time zone), `DATETIME` a date
/// and time without a zone; `ENUM` and `SET` are text; binary strings, `GEOMETRY` and `VECTOR`
/// are bytes.
pub(crate) fn value_kind(c: &Column) -> ValueKind {
    use ColumnType::*;
    match c.column_type() {
        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONG | MYSQL_TYPE_LONGLONG
        | MYSQL_TYPE_YEAR => ValueKind::Integer,
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => ValueKind::Decimal,
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => ValueKind::Float,
        MYSQL_TYPE_BIT => ValueKind::Bit,
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => ValueKind::Date,
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => ValueKind::Interval,
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => ValueKind::Timestamp,
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => ValueKind::TimestampTz,
        MYSQL_TYPE_JSON => ValueKind::Json,
        MYSQL_TYPE_GEOMETRY | MYSQL_TYPE_VECTOR => ValueKind::Bytes,
        MYSQL_TYPE_ENUM | MYSQL_TYPE_SET => ValueKind::Text,
        MYSQL_TYPE_VAR_STRING
        | MYSQL_TYPE_VARCHAR
        | MYSQL_TYPE_STRING
        | MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB => {
            let f = c.flags();
            if binary(c) && !f.contains(ColumnFlags::ENUM_FLAG) && !f.contains(ColumnFlags::SET_FLAG) {
                ValueKind::Bytes
            } else {
                ValueKind::Text
            }
        }
        MYSQL_TYPE_NULL | MYSQL_TYPE_TYPED_ARRAY | MYSQL_TYPE_UNKNOWN => ValueKind::Other,
    }
}

/// A value of column `c` as the app shows it; `None` for NULL.
pub(crate) fn display(c: &Column, v: Option<&Value>) -> Option<String> {
    let raw: &[u8] = match v? {
        Value::NULL => return None,
        Value::Bytes(b) => b,
        // The text protocol sends every value as text; anything else is shown as MySQL
        // writes it.
        other => return Some(other.as_sql(false)),
    };
    Some(match value_kind(c) {
        ValueKind::Bytes => hex(raw),
        ValueKind::Bit => bits(raw, c.column_length()),
        _ => match std::str::from_utf8(raw) {
            Ok(s) => s.to_string(),
            // Not UTF-8 (a column whose character set the session does not convert): its bytes.
            Err(_) => hex(raw),
        },
    })
}

/// `0x` and the bytes in hex, as MySQL's client shows binary data.
pub(crate) fn hex(raw: &[u8]) -> String {
    let mut s = String::with_capacity(2 + raw.len() * 2);
    s.push_str("0x");
    for b in raw {
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// A `BIT(width)` value (big-endian bytes) as a bit literal, `b'0101'`: `width` digits.
fn bits(raw: &[u8], width: u32) -> String {
    let all: String = raw.iter().map(|b| format!("{b:08b}")).collect();
    let width = (width as usize).clamp(1, all.len().max(1));
    let digits = if all.len() >= width { &all[all.len() - width..] } else { &all };
    format!("b'{digits}'")
}

#[cfg(test)]
mod tests;
