use super::*;
use mysql_async::consts::{ColumnFlags, ColumnType};

fn col(ty: ColumnType) -> Column {
    Column::new(ty).with_character_set(255)
}

fn bin(ty: ColumnType) -> Column {
    Column::new(ty).with_character_set(63).with_flags(ColumnFlags::BINARY_FLAG)
}

fn shown(c: &Column, raw: Option<&[u8]>) -> Option<String> {
    let v = raw.map(|b| Value::Bytes(b.to_vec())).unwrap_or(Value::NULL);
    display(c, Some(&v))
}

#[test]
fn types_are_named_as_mysql_names_them() {
    use ColumnType::*;
    let unsigned = |ty| col(ty).with_flags(ColumnFlags::UNSIGNED_FLAG);
    let cases = [
        (unsigned(MYSQL_TYPE_LONGLONG), "bigint unsigned"),
        (col(MYSQL_TYPE_LONG), "int"),
        (col(MYSQL_TYPE_TINY).with_column_length(1), "tinyint(1)"),
        (col(MYSQL_TYPE_TINY).with_column_length(4), "tinyint"),
        (col(MYSQL_TYPE_NEWDECIMAL), "decimal"),
        (col(MYSQL_TYPE_VAR_STRING), "varchar"),
        (bin(MYSQL_TYPE_VAR_STRING), "varbinary"),
        (bin(MYSQL_TYPE_STRING), "binary"),
        (col(MYSQL_TYPE_STRING).with_flags(ColumnFlags::ENUM_FLAG), "enum"),
        (col(MYSQL_TYPE_STRING).with_flags(ColumnFlags::SET_FLAG), "set"),
        (col(MYSQL_TYPE_BLOB).with_column_length(262_140), "text"),
        (col(MYSQL_TYPE_BLOB).with_column_length(4_294_967_295), "longtext"),
        (bin(MYSQL_TYPE_BLOB).with_column_length(255), "tinyblob"),
        (bin(MYSQL_TYPE_BLOB).with_column_length(16_777_215), "mediumblob"),
        (col(MYSQL_TYPE_JSON), "json"),
        (col(MYSQL_TYPE_DATETIME), "datetime"),
        (col(MYSQL_TYPE_TIMESTAMP), "timestamp"),
        (col(MYSQL_TYPE_TIME), "time"),
        (col(MYSQL_TYPE_YEAR), "year"),
        (col(MYSQL_TYPE_BIT), "bit"),
        (bin(MYSQL_TYPE_GEOMETRY), "geometry"),
    ];
    for (c, name) in cases {
        assert_eq!(type_name(&c), name);
    }
}

#[test]
fn kinds_follow_the_types() {
    use ColumnType::*;
    assert_eq!(value_kind(&col(MYSQL_TYPE_TINY).with_column_length(1)), ValueKind::Integer, "not a boolean");
    assert_eq!(value_kind(&col(MYSQL_TYPE_YEAR)), ValueKind::Integer);
    assert_eq!(value_kind(&col(MYSQL_TYPE_NEWDECIMAL)), ValueKind::Decimal);
    assert_eq!(value_kind(&col(MYSQL_TYPE_DOUBLE)), ValueKind::Float);
    assert_eq!(value_kind(&col(MYSQL_TYPE_TIME)), ValueKind::Interval);
    assert_eq!(value_kind(&col(MYSQL_TYPE_DATETIME)), ValueKind::Timestamp);
    assert_eq!(value_kind(&col(MYSQL_TYPE_TIMESTAMP)), ValueKind::TimestampTz);
    assert_eq!(value_kind(&col(MYSQL_TYPE_DATE)), ValueKind::Date);
    assert_eq!(value_kind(&col(MYSQL_TYPE_JSON)), ValueKind::Json);
    assert_eq!(value_kind(&col(MYSQL_TYPE_BIT)), ValueKind::Bit);
    assert_eq!(value_kind(&bin(MYSQL_TYPE_BLOB)), ValueKind::Bytes);
    assert_eq!(value_kind(&bin(MYSQL_TYPE_GEOMETRY)), ValueKind::Bytes);
    assert_eq!(value_kind(&col(MYSQL_TYPE_BLOB)), ValueKind::Text);
    assert_eq!(value_kind(&col(MYSQL_TYPE_STRING).with_flags(ColumnFlags::ENUM_FLAG)), ValueKind::Text);
    // An ENUM of a binary collation is still text.
    assert_eq!(value_kind(&bin(MYSQL_TYPE_STRING).with_flags(ColumnFlags::ENUM_FLAG)), ValueKind::Text);
}

#[test]
fn values_are_the_servers_text_and_bytes_are_hex() {
    use ColumnType::*;
    assert_eq!(
        shown(&col(MYSQL_TYPE_LONGLONG), Some(b"18446744073709551615")).as_deref(),
        Some("18446744073709551615")
    );
    assert_eq!(shown(&col(MYSQL_TYPE_DATE), Some(b"0000-00-00")).as_deref(), Some("0000-00-00"));
    assert_eq!(shown(&col(MYSQL_TYPE_TIME), Some(b"-838:59:59.000000")).as_deref(), Some("-838:59:59.000000"));
    // Korean and an emoji (escaped: no Hangul in the code).
    let text = "\u{d55c}\u{ad6d}\u{c5b4} \u{1f418}";
    assert_eq!(shown(&col(MYSQL_TYPE_VAR_STRING), Some(text.as_bytes())).as_deref(), Some(text));
    assert_eq!(shown(&bin(MYSQL_TYPE_BLOB), Some(&[0, 1, 0xab, 0xff])).as_deref(), Some("0x0001ABFF"));
    assert_eq!(shown(&bin(MYSQL_TYPE_BLOB), Some(&[])).as_deref(), Some("0x"));
    assert_eq!(shown(&col(MYSQL_TYPE_JSON), None), None);
    // Text that is not UTF-8 (a character set the session does not convert) is shown as its bytes.
    assert_eq!(shown(&col(MYSQL_TYPE_VAR_STRING), Some(&[0xff, 0x41])).as_deref(), Some("0xFF41"));
}

#[test]
fn bits_are_shown_with_as_many_digits_as_the_column_has() {
    let bit = |width| col(ColumnType::MYSQL_TYPE_BIT).with_column_length(width);
    assert_eq!(shown(&bit(4), Some(&[5])).as_deref(), Some("b'0101'"));
    assert_eq!(shown(&bit(1), Some(&[1])).as_deref(), Some("b'1'"));
    assert_eq!(shown(&bit(10), Some(&[0x02, 0x01])).as_deref(), Some("b'1000000001'"));
    assert_eq!(shown(&bit(64), Some(&[0xff; 8])).as_deref(), Some(format!("b'{}'", "1".repeat(64)).as_str()));
}
