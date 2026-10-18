use super::read::{self, Field};
use super::*;
use crate::driver::ArrayElement;

fn col<'a>(name: &'a str, kind: ValueKind, type_name: &'a str) -> ParquetColumn<'a> {
    ParquetColumn { name, kind, type_name }
}

/// The file of `rows` (written in chunks of two) of `columns`, as `setup` makes the writer, and
/// its row groups.
fn file<'a>(
    columns: &'a [ParquetColumn<'a>],
    rows: &[super::super::Row],
    setup: impl FnOnce(ParquetWriter<'a, Vec<u8>>) -> ParquetWriter<'a, Vec<u8>>,
) -> (read::File, u32) {
    let mut w = setup(ParquetWriter::new(columns, Vec::new()).unwrap());
    w.begin().unwrap();
    for chunk in rows.chunks(2) {
        w.rows(chunk).unwrap();
    }
    w.end().unwrap();
    assert_eq!(w.rows_written(), rows.len() as u64);
    let groups = w.groups();
    (read::file(&w.into_inner().unwrap()).expect("a Parquet file"), groups)
}

fn s(v: &str) -> Field {
    Field::Str(v.to_string())
}

/// Microseconds since 1970-01-01 of a UTC date and time.
fn micros(days: i64, h: i64, m: i64, sec: i64, us: i64) -> i64 {
    ((days * 24 + h) * 60 + m) * 60_000_000 + sec * 1_000_000 + us
}

#[test]
fn columns_have_their_types() {
    use ValueKind as K;
    let cases: &[(ValueKind, &str, Physical)] = &[
        (K::Integer, "int2", Physical::Int32),
        (K::Integer, "int4", Physical::Int32),
        (K::Integer, "int8", Physical::Int64),
        (K::Integer, "oid", Physical::Int64),
        (K::Integer, "tinyint(1)", Physical::Int32),
        (K::Integer, "smallint unsigned", Physical::Int32),
        (K::Integer, "mediumint unsigned", Physical::Int32),
        (K::Integer, "int", Physical::Int32),
        (K::Integer, "int unsigned", Physical::Int64),
        (K::Integer, "bigint", Physical::Int64),
        (K::Integer, "bigint unsigned", Physical::UInt64),
        (K::Integer, "year", Physical::Int32),
        (K::Float, "float4", Physical::Float),
        (K::Float, "float", Physical::Float),
        (K::Float, "float8", Physical::Double),
        (K::Float, "double", Physical::Double),
        (K::Bool, "bool", Physical::Bool),
        (K::Date, "date", Physical::Date),
        (K::Time, "time", Physical::Time),
        (K::Time, "timetz", Physical::Text),
        (K::Timestamp, "timestamp", Physical::Timestamp { utc: false }),
        (K::Timestamp, "datetime", Physical::Timestamp { utc: false }),
        (K::TimestampTz, "timestamptz", Physical::Timestamp { utc: true }),
        // MySQL's: the time shown in the session's zone, no offset.
        (K::TimestampTz, "timestamp", Physical::Timestamp { utc: false }),
        (K::Bytes, "bytea", Physical::Bytes),
        (K::Decimal, "numeric", Physical::Text),
        (K::Json, "jsonb", Physical::Text),
        (K::Text, "text", Physical::Text),
        (K::Bit, "varbit", Physical::Text),
        (K::Interval, "interval", Physical::Text),
        (K::Array(ArrayElement::Number), "int4[]", Physical::Text),
        (K::Other, "uuid", Physical::Text),
    ];
    for &(kind, name, want) in cases {
        assert_eq!(Physical::of(kind, name), want, "{name}");
    }
}

#[test]
fn values_read_back_with_their_types_and_nulls() {
    use ValueKind as K;
    let name = "\u{c774}\u{b984}";
    let columns = [
        col("small", K::Integer, "int4"),
        col("big", K::Integer, "int8"),
        col("unsigned", K::Integer, "bigint unsigned"),
        col("f4", K::Float, "float4"),
        col("f8", K::Float, "float8"),
        col("ok", K::Bool, "bool"),
        col(name, K::Text, "text"),
        col("day", K::Date, "date"),
        col("at", K::Time, "time"),
        col("local", K::Timestamp, "timestamp"),
        col("point", K::TimestampTz, "timestamptz"),
        col("price", K::Decimal, "numeric"),
        col("data", K::Bytes, "bytea"),
        col("doc", K::Json, "jsonb"),
        col("id", K::Other, "uuid"),
    ];
    let rows: Vec<super::super::Row> = vec![
        vec![
            Some("-2147483648"),
            Some("9223372036854775807"),
            Some("18446744073709551615"),
            Some("1.5"),
            Some("-0.25"),
            Some("t"),
            Some("\u{d55c}\u{ae00} \u{e9}"),
            Some("2024-02-29"),
            Some("23:59:59.999999"),
            Some("2024-01-02 03:04:05.5"),
            Some("2024-01-02 03:04:05+09"),
            Some("12345678901234567890.123456789"),
            Some("\\x00ff10"),
            Some("{\"a\": 1}"),
            Some("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"),
        ],
        vec![
            Some("7"),
            Some("-1"),
            Some("0"),
            Some("NaN"),
            Some("-Infinity"),
            Some("false"),
            Some(""),
            Some("0044-03-15 BC"),
            Some("00:00:00"),
            Some("1969-12-31 23:59:59"),
            Some("1970-01-01 05:30:00+05:30"),
            Some("-0.10"),
            Some("0x"),
            Some("[]"),
            Some("x"),
        ],
        vec![None; 15],
    ];
    let (f, groups) = file(&columns, &rows, |w| w);
    assert_eq!(groups, 1);
    assert_eq!(f.groups, vec![3]);
    let schema: Vec<(&str, PhysicalType, Option<LogicalType>)> =
        f.columns.iter().map(|c| (c.name.as_str(), c.physical, c.logical.clone())).collect();
    let utc = |u| LogicalType::Timestamp(TimestampType { is_adjusted_to_u_t_c: u, unit: TimeUnit::MICROS });
    assert_eq!(
        schema,
        vec![
            ("small", PhysicalType::INT32, None),
            ("big", PhysicalType::INT64, None),
            ("unsigned", PhysicalType::INT64, Some(LogicalType::Integer(IntType { bit_width: 64, is_signed: false }))),
            ("f4", PhysicalType::FLOAT, None),
            ("f8", PhysicalType::DOUBLE, None),
            ("ok", PhysicalType::BOOLEAN, None),
            (name, PhysicalType::BYTE_ARRAY, Some(LogicalType::String)),
            ("day", PhysicalType::INT32, Some(LogicalType::Date)),
            (
                "at",
                PhysicalType::INT64,
                Some(LogicalType::Time(TimeType { is_adjusted_to_u_t_c: false, unit: TimeUnit::MICROS }))
            ),
            ("local", PhysicalType::INT64, Some(utc(false))),
            ("point", PhysicalType::INT64, Some(utc(true))),
            ("price", PhysicalType::BYTE_ARRAY, Some(LogicalType::String)),
            ("data", PhysicalType::BYTE_ARRAY, None),
            ("doc", PhysicalType::BYTE_ARRAY, Some(LogicalType::String)),
            ("id", PhysicalType::BYTE_ARRAY, Some(LogicalType::String)),
        ]
    );
    assert!(f.columns.iter().all(|c| c.optional));
    assert_eq!(f.rows.len(), 3);
    let r = &f.rows[0];
    assert_eq!(r[0], Field::Int(i32::MIN));
    assert_eq!(r[1], Field::Long(i64::MAX));
    assert_eq!(r[2], Field::ULong(u64::MAX));
    assert_eq!(r[3], Field::Float(1.5));
    assert_eq!(r[4], Field::Double(-0.25));
    assert_eq!(r[5], Field::Bool(true));
    assert_eq!(r[6], s("\u{d55c}\u{ae00} \u{e9}"));
    assert_eq!(r[7], Field::Date(19_782));
    assert_eq!(r[8], Field::TimeMicros(86_399_999_999));
    assert_eq!(r[9], Field::TimestampMicros(micros(19_724, 3, 4, 5, 500_000)));
    // 03:04:05 at +09 is 18:04:05 UTC the day before.
    assert_eq!(r[10], Field::TimestampMicros(micros(19_723, 18, 4, 5, 0)));
    // Every digit of a decimal, as text.
    assert_eq!(r[11], s("12345678901234567890.123456789"));
    assert_eq!(r[12], Field::Bytes(vec![0x00, 0xff, 0x10].into()));
    assert_eq!(r[13], s("{\"a\": 1}"));
    assert_eq!(r[14], s("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"));
    let r = &f.rows[1];
    assert_eq!(r[0], Field::Int(7));
    assert_eq!(r[1], Field::Long(-1));
    assert_eq!(r[2], Field::ULong(0));
    assert!(matches!(r[3], Field::Float(n) if n.is_nan()));
    assert_eq!(r[4], Field::Double(f64::NEG_INFINITY));
    assert_eq!(r[5], Field::Bool(false));
    // The empty string is not NULL.
    assert_eq!(r[6], s(""));
    // 44 BC is the proleptic year -43.
    assert_eq!(r[7], Field::Date(-735_160));
    assert_eq!(r[8], Field::TimeMicros(0));
    assert_eq!(r[9], Field::TimestampMicros(-1_000_000));
    assert_eq!(r[10], Field::TimestampMicros(0));
    assert_eq!(r[11], s("-0.10"));
    assert_eq!(r[12], Field::Bytes(Vec::new().into()));
    assert!(f.rows[2].iter().all(|v| *v == Field::Null), "{:?}", f.rows[2]);
}

#[test]
fn dates_count_days_from_1970() {
    for (v, want) in [
        ("1970-01-01", Some(0)),
        ("1969-12-31", Some(-1)),
        ("2000-03-01", Some(11_017)),
        ("1900-03-01", Some(-25_508)),
        ("0001-01-01", Some(-719_162)),
        ("0001-12-31 BC", Some(-719_163)),
        ("10000-01-01", Some(2_932_897)),
        // Not dates Parquet holds, or not dates.
        ("infinity", None),
        ("-infinity", None),
        ("0000-00-00", None),
        ("2023-02-29", None),
        ("2024-13-01", None),
        ("2024-01-32", None),
        ("0000-01-01 BC", None),
        ("24-01-01", None),
        ("2024-01-01 ", None),
        ("99999999-01-01", None),
    ] {
        assert_eq!(parse_date(v), want, "{v}");
    }
}

#[test]
fn times_and_timestamps_parse_only_what_they_hold() {
    assert_eq!(parse_time("12:34:56.78"), Some(45_296_780_000));
    for v in ["24:00:00", "12:60:00", "1:00:00", "12:00:00+09", "12:00:00.1234567", "12:00"] {
        assert_eq!(parse_time(v), None, "{v}");
    }
    assert_eq!(parse_timestamp("1970-01-01 00:00:00-00:53:28", true), Some(3_208_000_000));
    assert_eq!(parse_timestamp("1970-01-01T00:00:01", false), Some(1_000_000));
    for (v, utc) in [
        ("infinity", true),
        ("-infinity", false),
        ("0000-00-00 00:00:00", false),
        // An offset where none is expected, or none where one is.
        ("2024-01-02 03:04:05+09", false),
        ("2024-01-02 03:04:05", true),
        ("2024-01-02", false),
        // Past what microseconds since 1970 in 64 bits hold.
        ("294277-01-01 00:00:00", false),
    ] {
        assert_eq!(parse_timestamp(v, utc), None, "{v}");
    }
}

#[test]
fn a_value_that_does_not_fit_fails_with_its_column_and_keeps_the_rows_before() {
    let columns = [col("id", ValueKind::Integer, "int4"), col("at", ValueKind::TimestampTz, "timestamptz")];
    let mut w = ParquetWriter::new(&columns, Vec::new()).unwrap().row_groups(2, GROUP_BYTES);
    w.begin().unwrap();
    w.rows(&[vec![Some("1"), Some("2024-01-02 03:04:05+00")], vec![Some("2"), None]]).unwrap();
    let e = w
        .rows(&[
            vec![Some("3"), Some("2024-01-02 03:04:05+00")],
            vec![Some("4"), Some("infinity")],
            vec![Some("5"), None],
        ])
        .unwrap_err();
    let want =
        BadValue { column: "at".to_string(), row: 4, value: "infinity".to_string(), parquet_type: "TIMESTAMP (UTC)" };
    assert_eq!(BadValue::of(&e), Some(&want));
    assert_eq!(e.to_string(), "a value of column at (row 4), infinity, does not fit its Parquet type TIMESTAMP (UTC)");
    // Nothing of the row that failed is kept: not its id either.
    assert_eq!(w.rows_written(), 3);
    w.end().unwrap();
    let f = read::file(&w.into_inner().unwrap()).unwrap();
    assert_eq!(f.groups, vec![2, 1]);
    let ids: Vec<&Field> = f.rows.iter().map(|r| &r[0]).collect();
    assert_eq!(ids, [&Field::Int(1), &Field::Int(2), &Field::Int(3)]);
    // An integer too wide for INT32, a long value shown cut.
    let mut w = ParquetWriter::new(&columns, Vec::new()).unwrap();
    let e = w.rows(&[vec![Some("2147483648"), None]]).unwrap_err();
    assert_eq!(BadValue::of(&e).map(|b| (b.column.as_str(), b.parquet_type)), Some(("id", "INT32")));
    let long = format!("x{}", "9".repeat(100));
    let e = w.rows(&[vec![Some(&long), None]]).unwrap_err();
    let shown = &BadValue::of(&e).unwrap().value;
    assert_eq!(shown.chars().count(), SHOWN_CHARS + 1);
    assert!(shown.ends_with('\u{2026}'));
    // Another error is not one.
    assert_eq!(BadValue::of(&io::Error::other("disk full")), None);
}

#[test]
fn rows_go_in_row_groups_of_bounded_rows_and_bytes() {
    let columns = [col("n", ValueKind::Integer, "int8"), col("t", ValueKind::Text, "text")];
    let texts: Vec<String> = (0..10).map(|i| format!("row {i}")).collect();
    let n: Vec<String> = (0..10).map(|i| i.to_string()).collect();
    let rows: Vec<super::super::Row> = (0..10).map(|i| vec![Some(n[i].as_str()), Some(texts[i].as_str())]).collect();
    let (f, groups) = file(&columns, &rows, |w| w.row_groups(4, GROUP_BYTES));
    assert_eq!((groups, f.groups.clone()), (3, vec![4, 4, 2]));
    let back: Vec<Vec<Field>> = (0..10).map(|i| vec![Field::Long(i), s(&format!("row {i}"))]).collect();
    assert_eq!(f.rows, back);
    // By bytes: each row keeps 2 + 8 and 2 + 5 + 8 bytes; three rows reach 75.
    let (f, _) = file(&columns, &rows, |w| w.row_groups(GROUP_ROWS, 75));
    assert_eq!(f.groups, vec![3, 3, 3, 1]);
    assert_eq!(f.rows, back);
}

#[test]
fn ending_early_makes_a_file_of_the_rows_so_far() {
    // What a cancel does: the rows written so far, then the end.
    let columns = [col("n", ValueKind::Integer, "int4")];
    let mut w = ParquetWriter::new(&columns, Vec::new()).unwrap().row_groups(2, GROUP_BYTES);
    w.begin().unwrap();
    w.rows(&[vec![Some("1")], vec![Some("2")], vec![Some("3")]]).unwrap();
    w.end().unwrap();
    // Ending again is nothing.
    w.end().unwrap();
    let f = read::file(&w.into_inner().unwrap()).unwrap();
    assert_eq!(f.groups, vec![2, 1]);
    assert_eq!(f.rows.len(), 3);
}

#[test]
fn no_rows_make_a_file_with_the_schema() {
    let columns = [col("a", ValueKind::Text, "text"), col("a", ValueKind::Integer, "int4")];
    let (f, groups) = file(&columns, &[], |w| w);
    assert_eq!(groups, 0);
    assert!(f.rows.is_empty());
    // Two columns of one name are kept apart.
    assert_eq!(f.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "a"]);
}

#[test]
fn bytes_must_be_hex() {
    assert_eq!(parse_bytes("\\xDEad"), Some(vec![0xde, 0xad]));
    assert_eq!(parse_bytes("0X01"), Some(vec![1]));
    for v in ["\\x0", "\\xzz", "abc", "\\001"] {
        assert_eq!(parse_bytes(v), None, "{v}");
    }
}

/// A sink that takes `n` bytes, then fails.
struct Full(usize);

impl Write for Full {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.0 < buf.len() {
            return Err(io::Error::new(io::ErrorKind::StorageFull, "disk full"));
        }
        self.0 -= buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_file_that_cannot_be_written_is_not_finished() {
    let columns = [col("t", ValueKind::Text, "text")];
    let big = "x".repeat(64 * 1024);
    let mut w = ParquetWriter::new(&columns, Full(1024)).unwrap().row_groups(1, GROUP_BYTES);
    w.begin().unwrap();
    let e = w.rows(&[vec![Some(&big)]]).unwrap_err();
    assert!(BadValue::of(&e).is_none());
    assert!(w.rows(&[vec![Some("more")]]).is_err());
    assert!(w.end().is_err());
    assert!(w.into_inner().is_none());
}
