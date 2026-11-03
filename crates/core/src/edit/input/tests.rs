use super::*;
use crate::driver::structure::{ColumnFill, StructureColumn};
use crate::edit::Exactness;
use crate::sql::dialect::MySqlMode;

const PG: Dialect = Dialect::Postgres;
const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });

fn column(type_name: &str, kind: ValueKind, labels: &[&str]) -> TableColumn {
    TableColumn {
        column: StructureColumn {
            name: "c".into(),
            type_name: type_name.into(),
            not_null: false,
            default: None,
            fill: ColumnFill::Default,
            enum_labels: labels.iter().map(|l| l.to_string()).collect(),
            collation: None,
        },
        kind,
        exact: Exactness::Exact,
        repeated: false,
    }
}

/// Each `(text, expected)` checked against a column of `type_name` and `kind`.
fn table(d: Dialect, type_name: &str, kind: ValueKind, cases: &[(&str, Result<(), Invalid>)]) {
    let c = column(type_name, kind, &[]);
    for (text, want) in cases {
        assert_eq!(check_input(d, &c, text), *want, "{d:?} {type_name} {text:?}");
    }
}

fn range(type_name: &str) -> Result<(), Invalid> {
    Err(Invalid::OutOfRange { type_name: type_name.into() })
}

const OK: Result<(), Invalid> = Ok(());

#[test]
fn an_enum_takes_its_labels_exactly() {
    for d in [PG, MY] {
        let c = column("mood", ValueKind::Other, &["sad", "ok", "It's"]);
        assert_eq!(check_input(d, &c, "ok"), OK);
        assert_eq!(check_input(d, &c, "It's"), OK);
        assert_eq!(check_input(d, &c, "OK"), Err(Invalid::NotLabel), "case counts");
        assert_eq!(check_input(d, &c, "ok "), Err(Invalid::NotLabel));
        assert_eq!(check_input(d, &c, ""), Err(Invalid::NotLabel));
    }
    // A MySQL SET has no labels in the structure: the server checks it.
    table(MY, "set('a','b')", ValueKind::Text, &[("a,b", OK), ("x", OK)]);
}

#[test]
fn whole_numbers_are_in_their_type_s_range() {
    let nwn = || Err(Invalid::NotWholeNumber);
    table(
        PG,
        "smallint",
        ValueKind::Integer,
        &[
            ("-32768", OK),
            ("32767", OK),
            ("+7", OK),
            ("007", OK),
            ("-32769", range("smallint")),
            ("32768", range("smallint")),
            ("", nwn()),
            ("1 ", nwn()),
            ("1.0", nwn()),
            ("1e3", nwn()),
            ("-", nwn()),
            ("\u{ff11}", nwn()),
        ],
    );
    table(PG, "integer", ValueKind::Integer, &[("-2147483648", OK), ("2147483648", range("integer"))]);
    table(
        PG,
        "bigint",
        ValueKind::Integer,
        &[
            ("9223372036854775807", OK),
            ("9223372036854775808", range("bigint")),
            ("99999999999999999999999999999999999999999", range("bigint")),
        ],
    );
    table(PG, "oid", ValueKind::Integer, &[("4294967295", OK), ("-1", range("oid"))]);
    table(PG, "mydomain", ValueKind::Integer, &[("99999999999999999999", OK), ("x", nwn())]);
    table(MY, "tinyint(1)", ValueKind::Integer, &[("1", OK), ("2", OK), ("-128", OK), ("128", range("tinyint(1)"))]);
    table(MY, "tinyint unsigned", ValueKind::Integer, &[("255", OK), ("256", range("tinyint unsigned"))]);
    table(MY, "tinyint unsigned", ValueKind::Integer, &[("-1", range("tinyint unsigned"))]);
    table(MY, "mediumint", ValueKind::Integer, &[("8388607", OK), ("8388608", range("mediumint"))]);
    table(MY, "int(11)", ValueKind::Integer, &[("-2147483648", OK), ("-2147483649", range("int(11)"))]);
    table(
        MY,
        "int(10) unsigned zerofill",
        ValueKind::Integer,
        &[("0000000042", OK), ("4294967296", range("int(10) unsigned zerofill"))],
    );
    table(
        MY,
        "bigint unsigned",
        ValueKind::Integer,
        &[("18446744073709551615", OK), ("18446744073709551616", range("bigint unsigned"))],
    );
    table(MY, "year", ValueKind::Integer, &[("2024", OK), ("0000", OK), ("1901", OK), ("24", range("year"))]);
    table(MY, "year", ValueKind::Integer, &[("2156", range("year")), ("-2000", range("year"))]);
}

#[test]
fn decimals_fit_their_precision_and_scale() {
    let many = || Err(Invalid::TooManyDigits { precision: 5, scale: 2 });
    for d in [PG, MY] {
        let ty = if matches!(d, Dialect::Postgres) { "numeric(5,2)" } else { "decimal(5,2)" };
        table(
            d,
            ty,
            ValueKind::Decimal,
            &[
                ("123.45", OK),
                ("-123.45", OK),
                ("1.50", OK),
                ("1.230", OK),
                ("000123.4", OK),
                ("0.00", OK),
                ("12", OK),
                ("1234.5", many()),
                ("1.234", many()),
                ("1e2", Err(Invalid::NotNumber)),
                ("1,5", Err(Invalid::NotNumber)),
                ("", Err(Invalid::NotNumber)),
                (".", Err(Invalid::NotNumber)),
            ],
        );
    }
    table(PG, "numeric(5,2)", ValueKind::Decimal, &[("NaN", OK), ("Infinity", Err(Invalid::NotNumber))]);
    table(PG, "numeric", ValueKind::Decimal, &[("NaN", OK), ("Infinity", OK), ("-Infinity", OK), ("1234567.891", OK)]);
    table(
        PG,
        "numeric(3)",
        ValueKind::Decimal,
        &[("999", OK), ("1.5", Err(Invalid::TooManyDigits { precision: 3, scale: 0 }))],
    );
    table(PG, "money", ValueKind::Decimal, &[("$1,000.00", OK)]);
    table(MY, "decimal(10,2)", ValueKind::Decimal, &[("NaN", Err(Invalid::NotNumber))]);
    table(
        MY,
        "decimal(10,2) unsigned",
        ValueKind::Decimal,
        &[("-1.00", range("decimal(10,2) unsigned")), ("-0.00", OK)],
    );
}

#[test]
fn floats_take_exponents_and_postgres_special_values() {
    for d in [PG, MY] {
        table(
            d,
            "double precision",
            ValueKind::Float,
            &[("1.5", OK), ("-0", OK), ("1e+15", OK), ("5e-324", OK), ("3.4028235e+38", OK), (".5", OK)],
        );
        table(d, "double", ValueKind::Float, &[("1e", Err(Invalid::NotNumber)), ("abc", Err(Invalid::NotNumber))]);
    }
    table(
        PG,
        "real",
        ValueKind::Float,
        &[("NaN", OK), ("Infinity", OK), ("-Infinity", OK), ("inf", Err(Invalid::NotNumber))],
    );
    table(MY, "double", ValueKind::Float, &[("NaN", Err(Invalid::NotNumber))]);
}

#[test]
fn booleans_are_true_or_false() {
    table(
        PG,
        "boolean",
        ValueKind::Bool,
        &[
            ("true", OK),
            ("false", OK),
            ("t", Err(Invalid::NotBool)),
            ("yes", Err(Invalid::NotBool)),
            ("TRUE", Err(Invalid::NotBool)),
        ],
    );
}

#[test]
fn dates_are_days_of_the_calendar() {
    let no = || Err(Invalid::NotDate);
    for d in [PG, MY] {
        table(
            d,
            "date",
            ValueKind::Date,
            &[
                ("2024-02-29", OK),
                ("2000-02-29", OK),
                ("2026-02-28", OK),
                ("2026-02-29", no()),
                ("1900-02-29", no()),
                ("2026-04-31", no()),
                ("2026-13-01", no()),
                ("2026-1-05", no()),
                ("2026-01-05 ", no()),
                ("20260105", no()),
                ("", no()),
            ],
        );
    }
    table(
        PG,
        "date",
        ValueKind::Date,
        &[
            ("infinity", OK),
            ("-infinity", OK),
            ("0044-03-15 BC", OK),
            ("0001-02-29 BC", OK),
            ("0002-02-29 BC", no()),
            ("12345-01-01", OK),
            ("0000-01-01", no()),
            ("0000-00-00", no()),
        ],
    );
    table(
        MY,
        "date",
        ValueKind::Date,
        &[("0000-00-00", OK), ("2024-00-15", OK), ("2024-02-00", OK), ("2024-13-00", no())],
    );
    table(MY, "date", ValueKind::Date, &[("infinity", no()), ("0044-03-15 BC", no()), ("12345-01-01", no())]);
}

#[test]
fn times_of_day_keep_their_type_s_digits_and_offset() {
    let no = || Err(Invalid::NotTime);
    table(
        PG,
        "time without time zone",
        ValueKind::Time,
        &[
            ("12:00:00", OK),
            ("23:59:59.999999", OK),
            ("00:00:00.5", OK),
            ("24:00:00", OK),
            ("24:00:01", no()),
            ("12:60:00", no()),
            ("12:00:60", no()),
            ("12:00", no()),
            ("12:00:00.", no()),
            ("12:00:00+09", no()),
            ("12:00:00.1234567", Err(Invalid::TooPrecise { digits: 6 })),
        ],
    );
    table(
        PG,
        "time(2) without time zone",
        ValueKind::Time,
        &[("12:00:00.25", OK), ("12:00:00.250", OK), ("12:00:00.125", Err(Invalid::TooPrecise { digits: 2 }))],
    );
    table(
        PG,
        "time with time zone",
        ValueKind::Time,
        &[
            ("12:00:00+09", OK),
            ("12:00:00.5-03:30", OK),
            ("12:00:00+08:27:52", OK),
            ("12:00:00", no()),
            ("12:00:00+9", no()),
            ("12:00:00+16", no()),
        ],
    );
}

#[test]
fn timestamps_are_shown_with_the_offset_their_type_has() {
    let no = || Err(Invalid::NotTimestamp);
    table(
        PG,
        "timestamp without time zone",
        ValueKind::Timestamp,
        &[
            ("2024-03-15 12:00:00", OK),
            ("2024-03-15 12:00:00.5", OK),
            ("0044-03-15 12:00:00 BC", OK),
            ("infinity", OK),
            ("-infinity", OK),
            ("2024-03-15T12:00:00", no()),
            ("2024-03-15 12:00:00+09", no()),
            ("2026-02-29 12:00:00", no()),
            ("2024-03-15", no()),
        ],
    );
    table(
        PG,
        "timestamp(0) without time zone",
        ValueKind::Timestamp,
        &[("2024-03-15 12:00:00.5", Err(Invalid::TooPrecise { digits: 0 }))],
    );
    table(
        PG,
        "timestamp with time zone",
        ValueKind::TimestampTz,
        &[
            ("2024-03-15 12:00:00+09", OK),
            ("2024-03-15 12:00:00.123456+05:30", OK),
            ("1900-01-01 00:00:00+08:27:52", OK),
            ("0044-03-15 12:00:00+00 BC", OK),
            ("infinity", OK),
            ("2024-03-15 12:00:00", no()),
            ("2024-03-15 12:00:00 +09", no()),
        ],
    );
    table(
        MY,
        "datetime",
        ValueKind::Timestamp,
        &[
            ("2024-03-15 12:00:00", OK),
            ("0000-00-00 00:00:00", OK),
            ("2024-03-15 12:00:00.5", Err(Invalid::TooPrecise { digits: 0 })),
            ("2024-03-15 24:00:00", no()),
            ("infinity", no()),
        ],
    );
    table(MY, "datetime(6)", ValueKind::Timestamp, &[("2024-03-15 12:00:00.500000", OK)]);
    table(
        MY,
        "datetime(3)",
        ValueKind::Timestamp,
        &[("2024-03-15 12:00:00.500", OK), ("2024-03-15 12:00:00.5001", Err(Invalid::TooPrecise { digits: 3 }))],
    );
    table(MY, "timestamp", ValueKind::TimestampTz, &[("2024-03-15 12:00:00", OK), ("2024-03-15 12:00:00+09:00", no())]);
}

#[test]
fn bytes_are_hexadecimal_as_the_result_shows_them() {
    table(
        PG,
        "bytea",
        ValueKind::Bytes,
        &[
            ("\\x", OK),
            ("\\xdeadbeef", OK),
            ("\\xDEADbeef", OK),
            ("\\xabc", Err(Invalid::NotBytes { mysql: false })),
            ("0xab", Err(Invalid::NotBytes { mysql: false })),
            ("abc", Err(Invalid::NotBytes { mysql: false })),
        ],
    );
    let no = || Err(Invalid::NotBytes { mysql: true });
    table(
        MY,
        "varbinary(16)",
        ValueKind::Bytes,
        &[("0x", OK), ("0xDEADBEEF", OK), ("0xab", no()), ("0xABC", no()), ("\\xAB", no()), ("AB", no())],
    );
}

#[test]
fn bits_have_their_column_s_count() {
    table(
        PG,
        "bit(4)",
        ValueKind::Bit,
        &[
            ("0101", OK),
            ("010", Err(Invalid::BitCount { count: 4, exact: true })),
            ("01010", Err(Invalid::BitCount { count: 4, exact: true })),
            ("0102", Err(Invalid::NotBits { mysql: false })),
            ("b'0101'", Err(Invalid::NotBits { mysql: false })),
        ],
    );
    table(
        PG,
        "bit varying(4)",
        ValueKind::Bit,
        &[("", OK), ("01", OK), ("01010", Err(Invalid::BitCount { count: 4, exact: false }))],
    );
    table(PG, "bit varying", ValueKind::Bit, &[("0101010101", OK)]);
    table(
        MY,
        "bit(4)",
        ValueKind::Bit,
        &[
            ("b'0101'", OK),
            ("b'101'", OK),
            ("b'00101'", OK),
            ("b'10101'", Err(Invalid::BitCount { count: 4, exact: false })),
            ("b''", Err(Invalid::NotBits { mysql: true })),
            ("0101", Err(Invalid::NotBits { mysql: true })),
            ("b'0102'", Err(Invalid::NotBits { mysql: true })),
        ],
    );
}

#[test]
fn uuids_are_hexadecimal_groups() {
    table(
        PG,
        "uuid",
        ValueKind::Other,
        &[
            ("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11", OK),
            ("A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11", OK),
            ("a0eebc999c0b4ef8bb6d6bb9bd380a11", Err(Invalid::NotUuid)),
            ("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a1g", Err(Invalid::NotUuid)),
            ("{a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11}", Err(Invalid::NotUuid)),
        ],
    );
    // Other types are the server's to check.
    table(PG, "inet", ValueKind::Other, &[("not an address", OK)]);
    table(MY, "uuid", ValueKind::Other, &[("x", OK)]);
}

#[test]
fn text_is_no_longer_than_its_column_s_characters() {
    let long = || Err(Invalid::TooLong { max: 3 });
    for (d, ty) in [(PG, "character varying(3)"), (PG, "character(3)"), (MY, "varchar(3)"), (MY, "char(3)")] {
        table(
            d,
            ty,
            ValueKind::Text,
            &[("abc", OK), ("", OK), ("abcd", long()), ("\u{e9}t\u{e9}", OK), ("\u{4e2d}\u{e9}\u{e9}", OK)],
        );
        table(d, ty, ValueKind::Text, &[("\u{e9}t\u{e9}s", long())]);
    }
    table(PG, "text", ValueKind::Text, &[("any length at all", OK)]);
    table(PG, "character varying", ValueKind::Text, &[("any length at all", OK)]);
    table(MY, "tinytext", ValueKind::Text, &[("any", OK)]);
}

#[test]
fn json_intervals_and_arrays_are_the_server_s_to_check() {
    table(PG, "jsonb", ValueKind::Json, &[("{not json", OK)]);
    table(PG, "interval", ValueKind::Interval, &[("1 year 2 mons", OK)]);
    table(MY, "time", ValueKind::Interval, &[("-838:59:59", OK)]);
    table(PG, "integer[]", ValueKind::Array(crate::driver::ArrayElement::Number), &[("{1,2}", OK)]);
}

fn rounded(type_name: &str, stored: &str) -> Result<(), Invalid> {
    Err(Invalid::Rounded { type_name: type_name.into(), stored: stored.into() })
}

#[test]
fn floats_are_taken_only_as_their_type_holds_them() {
    for ty in ["real", "float4", "float(24)"] {
        table(
            PG,
            ty,
            ValueKind::Float,
            &[
                ("0.123456789012", rounded(ty, "0.12345679")),
                ("16777217", rounded(ty, "16777216")),
                ("1e-50", rounded(ty, "0")),
                ("1e39", range(ty)),
                ("0.100000001", rounded(ty, "0.1")),
            ],
        );
    }
    for ty in ["float", "float(10)"] {
        table(MY, ty, ValueKind::Float, &[("0.123456789012", rounded(ty, "0.12345679"))]);
    }
    for (d, ty) in [(PG, "double precision"), (PG, "float8"), (PG, "float(25)"), (MY, "double"), (MY, "real")] {
        table(
            d,
            ty,
            ValueKind::Float,
            &[
                ("0.123456789012", OK),
                ("0.12345678901234567890", rounded(ty, "0.12345678901234568")),
                ("9007199254740993", rounded(ty, "9007199254740992")),
                ("1e309", range(ty)),
            ],
        );
    }
    table(
        MY,
        "double(5,2)",
        ValueKind::Float,
        &[("1.23", OK), ("1.230", OK), ("1.234", rounded("double(5,2)", "1.23")), ("1234", range("double(5,2)"))],
    );
    table(MY, "double unsigned", ValueKind::Float, &[("-1", range("double unsigned")), ("-0", OK), ("1", OK)]);
}

/// What the drivers show for a float (the shortest digits that read back to it; PostgreSQL's
/// in fixed notation in a range of exponents, else `1e+15`) is taken back as it is.
#[test]
fn floats_as_the_result_shows_them_are_taken_back() {
    let real = [
        "0.1",
        "-1.5",
        "-0",
        "0",
        "123456",
        "1.234567e+06",
        "3.4028235e+38",
        "-3.4028235e+38",
        "1.1754944e-38",
        "1e-45",
        "0.0001",
        "1e-05",
        "0.33333334",
        "16777216",
    ];
    for v in real {
        assert_eq!(check_input(PG, &column("real", ValueKind::Float, &[]), v), OK, "real {v}");
    }
    let double = [
        "0.1",
        "0.30000000000000004",
        "123456789012345",
        "1e+15",
        "1.7976931348623157e+308",
        "2.2250738585072014e-308",
        "5e-324",
        "-0",
        "1e-05",
        "3.141592653589793",
    ];
    for v in double {
        assert_eq!(check_input(PG, &column("double precision", ValueKind::Float, &[]), v), OK, "double {v}");
    }
    // MySQL's: a FLOAT to about six digits, a DOUBLE's shortest digits, with `e` and no sign.
    for v in ["3.14159", "3.40282e38", "1e-7", "-1.5"] {
        assert_eq!(check_input(MY, &column("float", ValueKind::Float, &[]), v), OK, "FLOAT {v}");
    }
    for v in ["0.1", "1.7976931348623157e308", "1e-7", "123456789.123"] {
        assert_eq!(check_input(MY, &column("double", ValueKind::Float, &[]), v), OK, "DOUBLE {v}");
    }
}

#[test]
fn mysql_char_columns_drop_trailing_spaces() {
    for ty in ["char(5)", "character(5)"] {
        table(MY, ty, ValueKind::Text, &[("ab ", Err(Invalid::TrailingSpaces)), (" ab", OK), ("ab", OK)]);
    }
    table(MY, "varchar(5)", ValueKind::Text, &[("ab ", OK)]);
    table(PG, "character(5)", ValueKind::Text, &[("ab ", OK)]);
}

#[test]
fn mysql_floats_of_fixed_digits_take_back_what_the_server_shows() {
    // DOUBLE(30,20) shows 0.1 with the digits of its binary value.
    let double = column("double(30,20)", ValueKind::Float, &[]);
    for v in ["0.10000000000000000555", "0.1", "0.5", "-0.10000000000000000555"] {
        assert_eq!(check_input(MY, &double, v), OK, "DOUBLE(30,20) {v}");
    }
    let rounded = |stored: &str| Err(Invalid::Rounded { type_name: "double(30,20)".into(), stored: stored.into() });
    // Read as 0.1, which the server shows otherwise.
    assert_eq!(check_input(MY, &double, "0.1000000000000000056"), rounded("0.10000000000000000555"));
    assert_eq!(check_input(MY, &double, "0.100000000000000005551"), rounded("0.10000000000000000555"));
    let float = column("float(20,15)", ValueKind::Float, &[]);
    assert_eq!(check_input(MY, &float, "0.100000001490116"), OK);
    assert!(matches!(check_input(MY, &float, "0.100000001490117"), Err(Invalid::Rounded { .. })));
}
