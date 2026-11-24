//! Values the user types into a cell (`edit::check_input`) and values a copy as SQL writes
//! (`export::sql_cells`), on each engine.

use crate::Engine::{MariaDb, MySql, Pg};
use crate::{Case, Engine, verdict};
use datarig_core::driver::structure::{ColumnFill, StructureColumn};
use datarig_core::driver::{ReadIn, SessionValues, ValueKind};
use datarig_core::edit::{Exactness, TableColumn, check_input};
use datarig_core::export::{Kind, SqlColumn, sql_cells, sql_value};
use datarig_core::sql::dialect::Dialect;

/// A value typed for a table column of a type.
pub struct Typed {
    pub type_name: &'static str,
    pub kind: ValueKind,
    pub labels: &'static [&'static str],
    pub text: &'static str,
}

/// Whether the typed value is taken on `e`.
pub fn input(e: Engine, t: &Typed) -> String {
    input_in(e.dialect(), t)
}

/// [`input`] in dialect `d`.
pub fn input_in(d: Dialect, t: &Typed) -> String {
    let column = TableColumn {
        column: StructureColumn {
            name: "c".to_string(),
            type_name: t.type_name.to_string(),
            not_null: false,
            default: None,
            fill: ColumnFill::Default,
            enum_labels: t.labels.iter().map(|l| l.to_string()).collect(),
            collation: None,
        },
        kind: t.kind,
        exact: Exactness::Exact,
        repeated: false,
    };
    verdict(&check_input(d, &column, t.text))
}

fn typed(type_name: &'static str, kind: ValueKind, text: &'static str) -> Typed {
    Typed { type_name, kind, labels: &[], text }
}

pub fn input_cases() -> Vec<Case<Typed>> {
    use ValueKind as V;
    vec![
        Case {
            name: "integer",
            input: typed("int", V::Integer, "12"),
            expect: &[(Pg, "ok"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "integer with a point",
            input: typed("int", V::Integer, "1.5"),
            expect: &[(Pg, "NotWholeNumber"), (MySql, "NotWholeNumber"), (MariaDb, "NotWholeNumber")],
        },
        Case {
            name: "tinyint out of range",
            input: typed("tinyint", V::Integer, "300"),
            expect: &[
                (Pg, "ok"),
                (MySql, "OutOfRange { type_name: \"tinyint\" }"),
                (MariaDb, "OutOfRange { type_name: \"tinyint\" }"),
            ],
        },
        Case {
            name: "smallint out of range",
            input: typed("smallint", V::Integer, "40000"),
            expect: &[
                (Pg, "OutOfRange { type_name: \"smallint\" }"),
                (MySql, "OutOfRange { type_name: \"smallint\" }"),
                (MariaDb, "OutOfRange { type_name: \"smallint\" }"),
            ],
        },
        Case {
            name: "negative unsigned",
            input: typed("int(11) unsigned", V::Integer, "-1"),
            expect: &[
                (Pg, "ok"),
                (MySql, "OutOfRange { type_name: \"int(11) unsigned\" }"),
                (MariaDb, "OutOfRange { type_name: \"int(11) unsigned\" }"),
            ],
        },
        Case {
            name: "decimal too many digits",
            input: typed("numeric(5,2)", V::Decimal, "123.456"),
            expect: &[
                (Pg, "TooManyDigits { precision: 5, scale: 2 }"),
                (MySql, "TooManyDigits { precision: 5, scale: 2 }"),
                (MariaDb, "TooManyDigits { precision: 5, scale: 2 }"),
            ],
        },
        Case {
            name: "decimal",
            input: typed("decimal(5,2)", V::Decimal, "123.45"),
            expect: &[(Pg, "ok"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "real",
            input: typed("real", V::Float, "0.1"),
            expect: &[(Pg, "ok"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "float",
            input: typed("float", V::Float, "0.1"),
            expect: &[(Pg, "ok"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "double",
            input: typed("double", V::Float, "0.1"),
            expect: &[(Pg, "ok"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "bool",
            input: typed("boolean", V::Bool, "true"),
            expect: &[(Pg, "ok"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "bool as a number",
            input: typed("boolean", V::Bool, "1"),
            expect: &[(Pg, "NotBool"), (MySql, "NotBool"), (MariaDb, "NotBool")],
        },
        Case {
            name: "day not in the calendar",
            input: typed("date", V::Date, "2026-02-30"),
            expect: &[(Pg, "NotDate"), (MySql, "NotDate"), (MariaDb, "NotDate")],
        },
        Case {
            name: "zero date",
            input: typed("date", V::Date, "0000-00-00"),
            expect: &[(Pg, "NotDate"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "timestamp",
            input: typed("timestamp", V::Timestamp, "2026-11-18 10:00:00"),
            expect: &[(Pg, "ok"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "timestamp with an offset",
            input: typed("timestamp with time zone", V::TimestampTz, "2026-11-18 10:00:00+00"),
            expect: &[(Pg, "ok"), (MySql, "NotTimestamp"), (MariaDb, "NotTimestamp")],
        },
        Case {
            name: "zoned timestamp without an offset",
            input: typed("timestamp", V::TimestampTz, "2026-11-18 10:00:00"),
            expect: &[(Pg, "NotTimestamp"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "timestamp too precise",
            input: typed("timestamp(3)", V::Timestamp, "2026-11-18 10:00:00.12345"),
            expect: &[
                (Pg, "TooPrecise { digits: 3 }"),
                (MySql, "TooPrecise { digits: 3 }"),
                (MariaDb, "TooPrecise { digits: 3 }"),
            ],
        },
        Case {
            name: "time past a day",
            input: typed("time", V::Interval, "838:59:59"),
            expect: &[(Pg, "ok"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "time of day",
            input: typed("time", V::Time, "25:00:00"),
            expect: &[(Pg, "NotTime"), (MySql, "NotTime"), (MariaDb, "NotTime")],
        },
        Case {
            name: "postgres bytes",
            input: typed("bytea", V::Bytes, "\\x0a"),
            expect: &[(Pg, "ok"), (MySql, "NotBytes { form: ZeroX }"), (MariaDb, "NotBytes { form: ZeroX }")],
        },
        Case {
            name: "mysql bytes",
            input: typed("varbinary(4)", V::Bytes, "0x0A"),
            expect: &[(Pg, "NotBytes { form: BackslashX }"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "bits",
            input: typed("bit(3)", V::Bit, "101"),
            expect: &[(Pg, "ok"), (MySql, "NotBits { form: BQuoted }"), (MariaDb, "NotBits { form: BQuoted }")],
        },
        Case {
            name: "mysql bits",
            input: typed("bit(3)", V::Bit, "b'101'"),
            expect: &[(Pg, "NotBits { form: Digits }"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "text too long",
            input: typed("varchar(3)", V::Text, "abcd"),
            expect: &[(Pg, "TooLong { max: 3 }"), (MySql, "TooLong { max: 3 }"), (MariaDb, "TooLong { max: 3 }")],
        },
        Case {
            name: "char with trailing spaces",
            input: typed("char(3)", V::Text, "ab "),
            expect: &[(Pg, "ok"), (MySql, "TrailingSpaces"), (MariaDb, "TrailingSpaces")],
        },
        Case {
            name: "uuid",
            input: typed("uuid", V::Other, "not-a-uuid"),
            expect: &[(Pg, "NotUuid"), (MySql, "ok"), (MariaDb, "ok")],
        },
        Case {
            name: "enum label",
            input: Typed { type_name: "enum('a','b')", kind: V::Text, labels: &["a", "b"], text: "A" },
            expect: &[(Pg, "NotLabel"), (MySql, "NotLabel"), (MariaDb, "NotLabel")],
        },
        Case {
            name: "json",
            input: typed("json", V::Json, "{"),
            expect: &[(Pg, "ok"), (MySql, "ok"), (MariaDb, "ok")],
        },
    ]
}

/// A result column copied as SQL, what its values were read in, and values of it to write
/// (`None`: NULL).
pub struct Copied {
    pub columns: &'static [SqlColumn<'static>; 1],
    pub read_in: ReadIn,
    pub server_zone: Option<&'static str>,
    pub values: &'static [Option<&'static str>],
}

/// How the column is copied as SQL on `e`: refused, or each of its values as the SQL literal a
/// copy writes (what `SqlCells::cell` takes, written by `sql_value` in the column's kind), or
/// why not.
pub fn cells(e: Engine, c: &Copied) -> String {
    cells_in(e.dialect(), c)
}

/// [`cells`] in dialect `d`.
pub fn cells_in(d: Dialect, c: &Copied) -> String {
    let kind = Kind::from(c.columns[0].kind);
    match sql_cells(d, c.columns, &c.read_in, c.server_zone) {
        Err(refused) => format!("refused {refused:?}"),
        Ok(cells) => c
            .values
            .iter()
            .map(|v| match v {
                None => sql_value(d, kind, None),
                Some(v) => match cells.cell(0, v) {
                    Ok(taken) => sql_value(d, kind, Some(&taken)),
                    Err(refused) => format!("{:?}", refused.why),
                },
            })
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

const fn column(name: &'static str, type_name: &'static str, kind: ValueKind) -> SqlColumn<'static> {
    SqlColumn { name, type_name, kind, auto_increment: false }
}

fn read_in(time_zone: &str, charset: &str) -> ReadIn {
    ReadIn::Values(SessionValues { time_zone: time_zone.to_string(), charset: charset.to_string() })
}

fn copied(
    columns: &'static [SqlColumn<'static>; 1],
    read_in: ReadIn,
    values: &'static [Option<&'static str>],
) -> Copied {
    Copied { columns, read_in, server_zone: None, values }
}

const FLOAT: &[SqlColumn; 1] = &[column("f", "float", ValueKind::Float)];
const DOUBLE: &[SqlColumn; 1] = &[column("f", "double", ValueKind::Float)];
const TIMESTAMP: &[SqlColumn; 1] = &[column("at", "timestamp", ValueKind::TimestampTz)];
const TEXT: &[SqlColumn; 1] = &[column("t", "varchar", ValueKind::Text)];
const BYTES: &[SqlColumn; 1] = &[column("b", "varbinary", ValueKind::Bytes)];
const BITS: &[SqlColumn; 1] = &[column("bits", "bit(3)", ValueKind::Bit)];
const BYTEA: &[SqlColumn; 1] = &[column("b", "bytea", ValueKind::Bytes)];
const BOOL: &[SqlColumn; 1] = &[column("ok", "boolean", ValueKind::Bool)];
const SERIAL: &[SqlColumn; 1] =
    &[SqlColumn { name: "id", type_name: "int", kind: ValueKind::Integer, auto_increment: true }];
const SERIAL_TEXT: &[SqlColumn; 1] =
    &[SqlColumn { name: "code", type_name: "varchar", kind: ValueKind::Text, auto_increment: true }];

pub fn cells_cases() -> Vec<Case<Copied>> {
    vec![
        Case {
            name: "float",
            input: copied(FLOAT, read_in("+00:00", "utf8mb4"), &[Some("0.1")]),
            expect: &[
                (Pg, "0.1"),
                (MySql, "refused Refused { column: \"f\", why: Float }"),
                (MariaDb, "refused Refused { column: \"f\", why: Float }"),
            ],
        },
        Case {
            name: "double",
            input: copied(DOUBLE, read_in("+00:00", "utf8mb4"), &[Some("0.1")]),
            expect: &[(Pg, "0.1"), (MySql, "0.1"), (MariaDb, "0.1")],
        },
        Case {
            name: "timestamp in a fixed offset",
            input: copied(
                TIMESTAMP,
                read_in("+05:30", "utf8mb4"),
                &[Some("2026-11-18 10:00:00"), Some("0000-00-00 00:00:00")],
            ),
            expect: &[
                (Pg, "'2026-11-18 10:00:00' | '0000-00-00 00:00:00'"),
                (MySql, "'2026-11-18 10:00:00+05:30' | '0000-00-00 00:00:00'"),
                (MariaDb, "'2026-11-18 10:00:00+05:30' | '0000-00-00 00:00:00'"),
            ],
        },
        Case {
            name: "timestamp in utc",
            input: copied(TIMESTAMP, read_in("UTC", "utf8mb4"), &[Some("2026-11-18 10:00:00")]),
            expect: &[
                (Pg, "'2026-11-18 10:00:00'"),
                (MySql, "'2026-11-18 10:00:00+00:00'"),
                (MariaDb, "'2026-11-18 10:00:00+00:00'"),
            ],
        },
        Case {
            name: "timestamp in the server's zone",
            input: Copied {
                columns: TIMESTAMP,
                read_in: read_in("SYSTEM", "utf8mb4"),
                server_zone: Some("-03:00"),
                values: &[Some("2026-11-18 10:00:00")],
            },
            expect: &[
                (Pg, "'2026-11-18 10:00:00'"),
                (MySql, "'2026-11-18 10:00:00-03:00'"),
                (MariaDb, "'2026-11-18 10:00:00-03:00'"),
            ],
        },
        Case {
            name: "timestamp in a zone with daylight saving",
            input: copied(TIMESTAMP, read_in("Europe/Berlin", "utf8mb4"), &[Some("2026-11-18 10:00:00")]),
            expect: &[
                (Pg, "'2026-11-18 10:00:00'"),
                (MySql, "refused Refused { column: \"at\", why: TimeZone(\"Europe/Berlin\") }"),
                (MariaDb, "refused Refused { column: \"at\", why: TimeZone(\"Europe/Berlin\") }"),
            ],
        },
        Case {
            name: "timestamp read in unknown values",
            input: copied(TIMESTAMP, ReadIn::Unknown, &[Some("2026-11-18 10:00:00")]),
            expect: &[
                (Pg, "'2026-11-18 10:00:00'"),
                (MySql, "refused Refused { column: \"at\", why: SessionUnknown }"),
                (MariaDb, "refused Refused { column: \"at\", why: SessionUnknown }"),
            ],
        },
        Case {
            name: "timestamp read in changed values",
            input: copied(TIMESTAMP, ReadIn::Changed, &[Some("2026-11-18 10:00:00")]),
            expect: &[
                (Pg, "'2026-11-18 10:00:00'"),
                (MySql, "refused Refused { column: \"at\", why: SessionChanged }"),
                (MariaDb, "refused Refused { column: \"at\", why: SessionChanged }"),
            ],
        },
        Case {
            name: "text in utf8mb4",
            input: copied(TEXT, read_in("+00:00", "utf8mb4"), &[Some("it's")]),
            expect: &[(Pg, "'it''s'"), (MySql, "'it''s'"), (MariaDb, "'it''s'")],
        },
        Case {
            name: "text in latin1",
            input: copied(TEXT, read_in("+00:00", "latin1"), &[Some("x")]),
            expect: &[(Pg, "'x'"), (MySql, "Charset(\"latin1\")"), (MariaDb, "Charset(\"latin1\")")],
        },
        Case {
            name: "text read in unknown values",
            input: copied(TEXT, ReadIn::Unknown, &[Some("x")]),
            expect: &[(Pg, "'x'"), (MySql, "SessionUnknown"), (MariaDb, "SessionUnknown")],
        },
        Case {
            name: "text read in changed values",
            input: copied(TEXT, ReadIn::Changed, &[Some("x")]),
            expect: &[(Pg, "'x'"), (MySql, "SessionChanged"), (MariaDb, "SessionChanged")],
        },
        Case {
            name: "bytes",
            input: copied(BYTES, read_in("+00:00", "utf8mb4"), &[Some("0x0A"), Some("\\x0a")]),
            expect: &[
                (Pg, "'0x0A' | E'\\\\x0a'"),
                (MySql, "X'0A' | NotShownWhole"),
                (MariaDb, "X'0A' | NotShownWhole"),
            ],
        },
        Case {
            name: "auto_increment",
            input: copied(SERIAL, read_in("+00:00", "utf8mb4"), &[Some("0"), Some("5")]),
            expect: &[(Pg, "0 | 5"), (MySql, "AutoIncrementZero | 5"), (MariaDb, "AutoIncrementZero | 5")],
        },
        Case {
            name: "auto_increment read in unknown values",
            input: copied(SERIAL, ReadIn::Unknown, &[Some("0")]),
            expect: &[(Pg, "0"), (MySql, "AutoIncrementZero"), (MariaDb, "AutoIncrementZero")],
        },
        Case {
            name: "auto_increment text",
            input: copied(SERIAL_TEXT, read_in("+00:00", "utf8mb4"), &[Some("0")]),
            expect: &[(Pg, "'0'"), (MySql, "AutoIncrementZero"), (MariaDb, "AutoIncrementZero")],
        },
        Case {
            name: "null",
            input: copied(TEXT, read_in("+00:00", "utf8mb4"), &[None]),
            expect: &[(Pg, "NULL"), (MySql, "NULL"), (MariaDb, "NULL")],
        },
        Case {
            name: "negative zero",
            input: copied(DOUBLE, read_in("+00:00", "utf8mb4"), &[Some("-0"), Some("-0.0"), Some("0"), None]),
            expect: &[
                (Pg, "'-0' | '-0.0' | 0 | NULL"),
                (MySql, "'-0' | '-0.0' | 0 | NULL"),
                (MariaDb, "'-0' | '-0.0' | 0 | NULL"),
            ],
        },
        Case {
            name: "bits",
            input: copied(BITS, read_in("+00:00", "utf8mb4"), &[Some("b'101'"), Some("101"), None]),
            expect: &[
                (Pg, "'b''101''' | '101' | NULL"),
                (MySql, "b'101' | NotShownWhole | NULL"),
                (MariaDb, "b'101' | NotShownWhole | NULL"),
            ],
        },
        Case {
            name: "postgres bytes",
            input: copied(BYTEA, read_in("+00:00", "utf8mb4"), &[Some("\\x0a")]),
            expect: &[(Pg, "E'\\\\x0a'"), (MySql, "NotShownWhole"), (MariaDb, "NotShownWhole")],
        },
        Case {
            name: "bool",
            input: copied(BOOL, read_in("+00:00", "utf8mb4"), &[Some("true"), Some("1")]),
            expect: &[(Pg, "true | '1'"), (MySql, "true | '1'"), (MariaDb, "true | '1'")],
        },
    ]
}
