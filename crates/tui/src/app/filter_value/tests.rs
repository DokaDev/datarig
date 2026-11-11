use super::*;
use datarig_core::driver::SessionValues;

const MY: Dialect = Dialect::MySql(datarig_core::sql::dialect::MySqlMode {
    ansi_quotes: false,
    no_backslash_escapes: false,
    dollar_quotes: false,
});

fn meta(name: &str, ty: &str, kind: ValueKind) -> ColumnMeta {
    ColumnMeta::new(name.into(), ty.into(), kind, None)
}

fn session(zone: &str, charset: &str) -> SessionValues {
    SessionValues { time_zone: zone.into(), charset: charset.into() }
}

/// Value `v` of a column `m` read in `read_in`, with the session now in `now`.
fn judged(
    d: Dialect,
    m: &ColumnMeta,
    read_in: &ReadIn,
    now: Option<&SessionValues>,
    v: &str,
) -> Result<Value, NotAdmitted> {
    admit(&ValuesFrom { dialect: d, meta: m, read_in }, None, now, v)
}

fn not_whole(column: &str, why: export::NotWhole) -> Result<Value, NotAdmitted> {
    Err(NotAdmitted::NotWhole(export::Refused { column: column.into(), why }))
}

#[test]
fn values_are_taken_as_the_session_reads_them_back() {
    let utc = session("+00:00", "utf8mb4");
    let read = ReadIn::Values(utc.clone());
    let text = meta("name", "varchar", ValueKind::Text);
    assert_eq!(judged(MY, &text, &read, Some(&utc), "a'b"), Ok(Value::Text("a'b".into())));
    let int = meta("id", "int", ValueKind::Integer);
    assert_eq!(judged(MY, &int, &read, Some(&utc), "12"), Ok(Value::Number("12".into())));
    let bytes = meta("data", "varbinary", ValueKind::Bytes);
    assert_eq!(judged(MY, &bytes, &read, Some(&utc), "0x01AB"), Ok(Value::Bytes(vec![1, 0xab])));
    let pg = meta("data", "bytea", ValueKind::Bytes);
    assert_eq!(judged(Dialect::Postgres, &pg, &ReadIn::Unknown, None, "\\x01ab"), Ok(Value::Bytes(vec![1, 0xab])));
    // PostgreSQL reads every value back as shown.
    let tz = meta("at", "timestamptz", ValueKind::TimestampTz);
    assert_eq!(
        judged(Dialect::Postgres, &tz, &ReadIn::Unknown, None, "2026-11-10 09:30:00+09"),
        Ok(Value::Text("2026-11-10 09:30:00+09".into()))
    );
}

#[test]
fn a_rounded_float_is_not_taken() {
    let utc = session("+00:00", "utf8mb4");
    let w = meta("w", "float", ValueKind::Float);
    assert_eq!(
        judged(MY, &w, &ReadIn::Values(utc.clone()), Some(&utc), "1.5"),
        not_whole("w", export::NotWhole::Float)
    );
    // A DOUBLE is sent whole.
    let d = meta("d", "double", ValueKind::Float);
    assert_eq!(judged(MY, &d, &ReadIn::Values(utc.clone()), Some(&utc), "1.5"), Ok(Value::Number("1.5".into())));
}

#[test]
fn text_read_in_another_character_set_is_not_taken() {
    let latin = session("+00:00", "latin1");
    let text = meta("name", "varchar", ValueKind::Text);
    assert_eq!(
        judged(MY, &text, &ReadIn::Values(latin.clone()), Some(&latin), "abc"),
        not_whole("name", export::NotWhole::Charset("latin1".into()))
    );
    // Nor text read before the session said its values, or across a change of them.
    let utc = session("+00:00", "utf8mb4");
    assert_eq!(
        judged(MY, &text, &ReadIn::Unknown, Some(&utc), "abc"),
        not_whole("name", export::NotWhole::SessionUnknown)
    );
    assert_eq!(
        judged(MY, &text, &ReadIn::Changed, Some(&utc), "abc"),
        not_whole("name", export::NotWhole::SessionChanged)
    );
}

#[test]
fn a_timestamp_is_taken_only_in_the_zone_it_was_read_in() {
    let at = meta("at", "timestamp", ValueKind::TimestampTz);
    let utc = session("+00:00", "utf8mb4");
    let seoul = session("+09:00", "utf8mb4");
    let read = ReadIn::Values(utc.clone());
    let v = "2026-11-10 09:30:00";
    assert_eq!(judged(MY, &at, &read, Some(&utc), v), Ok(Value::Text(v.into())));
    // The zone changed since (an earlier load's values too: read in another zone).
    assert_eq!(judged(MY, &at, &read, Some(&seoul), v), not_whole("at", export::NotWhole::SessionChanged));
    assert_eq!(judged(MY, &at, &read, None, v), not_whole("at", export::NotWhole::SessionChanged));
    // A zone that is not a fixed offset: two instants in the hour a clock goes back.
    let berlin = session("Europe/Berlin", "utf8mb4");
    assert_eq!(
        judged(MY, &at, &ReadIn::Values(berlin.clone()), Some(&berlin), v),
        not_whole("at", export::NotWhole::TimeZone("Europe/Berlin".into()))
    );
}

#[test]
fn bytes_must_be_shown_whole_and_small() {
    let utc = session("+00:00", "utf8mb4");
    let bytes = meta("data", "varbinary", ValueKind::Bytes);
    let read = ReadIn::Values(utc.clone());
    assert_eq!(judged(MY, &bytes, &read, Some(&utc), "0x1"), not_whole("data", export::NotWhole::NotShownWhole));
    let big = format!("0x{}", "AB".repeat(MAX_BYTES + 1));
    assert_eq!(judged(MY, &bytes, &read, Some(&utc), &big), Err(NotAdmitted::TooBig));
    let edge = format!("0x{}", "AB".repeat(MAX_BYTES));
    assert!(judged(MY, &bytes, &read, Some(&utc), &edge).is_ok());
}
